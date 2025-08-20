// src/bin/execution_bot.rs

use anyhow::{anyhow, Result};
use futures_util::StreamExt;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use mev::{
    config::Config,
    decoders::{Pool, PoolOperations},
    graph_engine::Graph,
};
use solana_client::{
    nonblocking::pubsub_client::PubsubClient,
    rpc_config::RpcAccountInfoConfig,
};
use solana_account_decoder::{UiAccountEncoding, UiAccountData};
use solana_program_pack::Pack;
use solana_sdk::{account::Account, pubkey::Pubkey};
use spl_token::state::Account as SplTokenAccount;
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::Read,
    str::FromStr,
    sync::{Arc, Mutex},
};
use tokio::{sync::mpsc, task::JoinHandle, time}; // <-- time ajouté pour sleep
use mev::strategies::spatial::find_spatial_arbitrage;


// ... load_main_graph_from_cache, read_hotlist, subscribe_to_hot_vault ne changent pas ...
fn load_main_graph_from_cache() -> Result<Graph> {
    println!("[Graph] Chargement du cache de pools de référence depuis 'graph_cache.bin'...");
    let mut file = std::fs::File::open("graph_cache.bin")?;
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer)?;
    let pools: HashMap<Pubkey, Pool> = bincode::deserialize(&buffer)?;
    Ok(Graph { pools, account_to_pool_map: HashMap::new() })
}

fn read_hotlist() -> Result<HashSet<Pubkey>> {
    let data = fs::read_to_string("hotlist.json")?;
    let hotlist: HashSet<Pubkey> = serde_json::from_str(&data)?;
    Ok(hotlist)
}

async fn subscribe_to_hot_vault(pubsub_client: Arc<PubsubClient>, vault_address: Pubkey, update_sender: mpsc::Sender<(Pubkey, u64)>) {
    let config = RpcAccountInfoConfig { encoding: Some(UiAccountEncoding::Base64), ..Default::default() };
    let (mut stream, _unsubscribe) = pubsub_client.account_subscribe(&vault_address, Some(config)).await.unwrap();
    while let Some(response) = stream.next().await {
        let ui_account = response.value;
        let data = match ui_account.data {
            UiAccountData::Binary(encoded_data, _) => STANDARD.decode(encoded_data).unwrap_or_default(),
            _ => continue,
        };
        if let Ok(token_account) = SplTokenAccount::unpack(&data) {
            if update_sender.send((vault_address, token_account.amount)).await.is_err() { break; }
        }
    }
}

// --- NOUVELLE FONCTION: Pour mettre à jour le hot_graph ---
fn update_hot_graph_reserve(graph: &Arc<Mutex<Graph>>, vault_address: &Pubkey, new_balance: u64) {
    let mut graph_guard = graph.lock().unwrap();

    // On a besoin de recréer le account_to_pool_map pour le hot_graph
    // Note: C'est un peu inefficace, mais suffisant pour le dev.
    // En prod, on optimiserait ça.
    let mut temp_map = HashMap::new();
    for (pool_addr, pool) in &graph_guard.pools {
        let (v_a, v_b) = pool.get_vaults();
        temp_map.insert(v_a, *pool_addr);
        temp_map.insert(v_b, *pool_addr);
    }

    if let Some(pool_address) = temp_map.get(vault_address) {
        if let Some(pool) = graph_guard.pools.get_mut(pool_address) {
            let (vault_a, _vault_b) = pool.get_vaults();
            match pool {
                Pool::RaydiumAmmV4(p) => { if vault_address == &vault_a { p.reserve_a = new_balance } else { p.reserve_b = new_balance }; },
                Pool::RaydiumCpmm(p) => { if vault_address == &vault_a { p.reserve_a = new_balance } else { p.reserve_b = new_balance }; },
                Pool::PumpAmm(p) => { if vault_address == &vault_a { p.reserve_a = new_balance } else { p.reserve_b = new_balance }; },
                // Pour les CLMM etc., la mise à jour est plus complexe, on le fera plus tard
                _ => {}
            }
        }
    }
}


#[tokio::main]
async fn main() -> Result<()> {
    println!("--- Lancement de l'Execution Bot (Version de Développement) ---");

    let config = Config::load()?;
    let rpc_client = Arc::new(RpcClient::new(config.solana_rpc_url));
    let main_graph = load_main_graph_from_cache()?;
    let hot_graph = Arc::new(Mutex::new(Graph::new()));
    let wss_url = config.solana_rpc_url.replace("http", "ws");
    let pubsub_client = Arc::new(PubsubClient::new(&wss_url).await?);
    let (update_sender, mut update_receiver) = mpsc::channel::<(Pubkey, u64)>(100);

    // --- DÉBUT DE LA NOUVELLE STRUCTURE ASYNCHRONE ---

    // Tâche 1: Gérer la mise à jour de la hotlist en arrière-plan
    let hot_graph_clone_for_task1 = Arc::clone(&hot_graph);
    let main_graph_clone_for_task1 = Arc::new(main_graph); // `main_graph` est en lecture seule, Arc suffit
    let pubsub_client_clone_for_task1 = Arc::clone(&pubsub_client);
    let update_sender_clone_for_task1 = update_sender.clone();
    tokio::spawn(async move {
        let mut subscription_handles: HashMap<Pubkey, JoinHandle<()>> = HashMap::new();
        let mut current_subscribed_vaults = HashSet::new();
        loop {
            if let Ok(hotlist_pools) = read_hotlist() {
                let mut target_vaults = HashSet::new();
                for pool_key in &hotlist_pools {
                    if let Some(pool_ref) = main_graph_clone_for_task1.pools.get(pool_key) {
                        let (vault_a, vault_b) = pool_ref.get_vaults();
                        target_vaults.insert(vault_a);
                        target_vaults.insert(vault_b);
                    }
                }

                let vaults_to_add: HashSet<_> = target_vaults.difference(&current_subscribed_vaults).cloned().collect();
                let vaults_to_remove: HashSet<_> = current_subscribed_vaults.difference(&target_vaults).cloned().collect();

                if !vaults_to_add.is_empty() || !vaults_to_remove.is_empty() {
                    println!("[Hotlist] Changement détecté. Ajout: {}, Suppression: {}.", vaults_to_add.len(), vaults_to_remove.len());
                    for vault_key in &vaults_to_remove { if let Some(handle) = subscription_handles.remove(vault_key) { handle.abort(); } }
                    for vault_key in &vaults_to_add {
                        let handle = tokio::spawn(subscribe_to_hot_vault(Arc::clone(&pubsub_client_clone_for_task1), *vault_key, update_sender_clone_for_task1.clone()));
                        subscription_handles.insert(*vault_key, handle);
                    }
                    let mut new_hot_graph = Graph::new();
                    for pool_key in &hotlist_pools {
                        if let Some(pool_ref) = main_graph_clone_for_task1.pools.get(pool_key) { new_hot_graph.add_pool_to_graph(pool_ref.clone()); }
                    }
                    *hot_graph_clone_for_task1.lock().unwrap() = new_hot_graph;
                    current_subscribed_vaults = target_vaults;
                    println!("[Hotlist] Abonnements actifs: {}", current_subscribed_vaults.len());
                }
            }
            time::sleep(time::Duration::from_secs(5)).await;
        }
    });

    // Tâche 2: Boucle principale pour traiter les mises à jour et chercher des arbitrages
    println!("[Bot] Prêt. En attente d'opportunités d'arbitrage...");
    loop {
        if let Some((vault_address, new_balance)) = update_receiver.recv().await {
            println!("[Arbitrage] Changement de réserve sur {}. Nouveau solde: {}. Recherche...", vault_address, new_balance);

            update_hot_graph_reserve(&hot_graph, &vault_address, new_balance);

            // On clone les Arcs pour les passer à la tâche de recherche
            let graph_clone = Arc::clone(&hot_graph);
            let client_clone = Arc::clone(&rpc_client);

            // On lance la recherche dans une tâche séparée pour ne pas bloquer la réception de nouvelles mises à jour
            tokio::spawn(async move {
                find_spatial_arbitrage(graph_clone, client_clone).await;
            });
        }
    }