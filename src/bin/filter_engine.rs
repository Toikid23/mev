// src/bin/filter_engine.rs

use anyhow::Result;
use chrono::Utc;
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
    collections::HashMap,
    fs,
    io::Read,
    str::FromStr,
    sync::{Arc, Mutex},
};
use tokio;
use std::collections::HashSet;

// ... load_graph_from_cache et update_pool_activity ne changent pas ...
fn load_graph_from_cache() -> Result<Graph> {
    println!("[Graph] Chargement du cache de pools depuis 'graph_cache.bin'...");
    let mut file = std::fs::File::open("graph_cache.bin")?;
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer)?;
    let pools: HashMap<Pubkey, Pool> = bincode::deserialize(&buffer)?;
    if pools.is_empty() { println!("[AVERTISSEMENT] Le cache de pools est vide."); }
    else { println!("[Graph] {} pools chargées avec succès.", pools.len()); }
    let mut account_to_pool_map = HashMap::new();
    for (pool_address, pool) in pools.iter() {
        let (vault_a, vault_b) = pool.get_vaults();
        account_to_pool_map.insert(vault_a, *pool_address);
        account_to_pool_map.insert(vault_b, *pool_address);
    }
    Ok(Graph { pools, account_to_pool_map })
}

fn update_pool_activity(graph: &Arc<Mutex<Graph>>, vault_address: &Pubkey, new_balance: u64, timestamp: i64) {
    let mut graph_guard = graph.lock().unwrap();
    let pool_address_clone = match graph_guard.account_to_pool_map.get(vault_address) {
        Some(addr) => *addr,
        None => return,
    };
    if let Some(pool) = graph_guard.pools.get_mut(&pool_address_clone) {
        let (vault_a, _vault_b) = pool.get_vaults();
        match pool {
            Pool::RaydiumAmmV4(p) => { if vault_address == &vault_a { p.reserve_a = new_balance } else { p.reserve_b = new_balance }; p.last_swap_timestamp = timestamp; }
            Pool::RaydiumCpmm(p) => { if vault_address == &vault_a { p.reserve_a = new_balance } else { p.reserve_b = new_balance }; p.last_swap_timestamp = timestamp; }
            Pool::RaydiumClmm(p) => { p.last_swap_timestamp = timestamp; }
            Pool::MeteoraDammV1(p) => { if vault_address == &vault_a { p.reserve_a = new_balance } else { p.reserve_b = new_balance }; p.last_swap_timestamp = timestamp; }
            Pool::MeteoraDammV2(p) => { p.last_swap_timestamp = timestamp; }
            Pool::MeteoraDlmm(p) => { p.last_swap_timestamp = timestamp; }
            Pool::OrcaWhirlpool(p) => { p.last_swap_timestamp = timestamp; }
            Pool::PumpAmm(p) => { if vault_address == &vault_a { p.reserve_a = new_balance } else { p.reserve_b = new_balance }; p.last_swap_timestamp = timestamp; }
            _ => {}
        }
    }
}

async fn subscribe_to_vault(pubsub_client: Arc<PubsubClient>, vault_address: Pubkey, graph: Arc<Mutex<Graph>>) {
    let config = RpcAccountInfoConfig { encoding: Some(UiAccountEncoding::Base64), ..Default::default() };
    let (mut account_stream, _unsubscribe) = pubsub_client.account_subscribe(&vault_address, Some(config)).await.unwrap();
    while let Some(response) = account_stream.next().await {
        let ui_account = response.value;
        let data = match ui_account.data {
            UiAccountData::Binary(encoded_data, _) => STANDARD.decode(encoded_data).unwrap_or_default(),
            _ => continue,
        };
        let owner = Pubkey::from_str(&ui_account.owner).unwrap();
        let account_data = Account { lamports: ui_account.lamports, data, owner, executable: ui_account.executable, rent_epoch: ui_account.rent_epoch };
        if let Ok(token_account) = SplTokenAccount::unpack(&account_data.data) {
            let timestamp = Utc::now().timestamp();
            println!("[WebSocket] Swap détecté sur le vault {} à {} (Solde: {})", vault_address, timestamp, token_account.amount);
            update_pool_activity(&graph, &vault_address, token_account.amount, timestamp);
        }
    }
}


#[tokio::main]
async fn main() -> Result<()> {
    println!("--- Lancement du Filter Engine (Version de Développement) ---");

    let config = Config::load()?;
    let graph = load_graph_from_cache()?;
    let shared_graph = Arc::new(Mutex::new(graph));

    let wss_url = config.solana_rpc_url.replace("http", "ws");
    println!("[WebSocket] Connexion à: {}", wss_url);
    let pubsub_client = Arc::new(PubsubClient::new(&wss_url).await?);

    let vault_pubkeys: Vec<Pubkey> = shared_graph.lock().unwrap().account_to_pool_map.keys().cloned().collect();
    println!("[WebSocket] Lancement des abonnements pour {} vaults...", vault_pubkeys.len());

    for vault_address in vault_pubkeys {
        let client_clone = Arc::clone(&pubsub_client);
        let graph_clone = Arc::clone(&shared_graph);
        tokio::spawn(async move {
            subscribe_to_vault(client_clone, vault_address, graph_clone).await;
        });
    }
    println!("[INFO] Tous les abonnements sont actifs. Le moteur de filtrage est en cours d'exécution.");

    // --- NOUVEAU: Constantes de filtrage ---
    const ACTIVITY_WINDOW_SECONDS: i64 = 60; // Fenêtre d'activité : 1 minute
    const MIN_SOL_LIQUIDITY: u64 = 100 * 1_000_000_000; // Seuil de liquidité : 100 SOL (en lamports)
    const HOTLIST_SIZE: usize = 10;
    let wsol_mint = Pubkey::from_str("So11111111111111111111111111111111111111112")?;

    loop {
        tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
        let now = Utc::now().timestamp();
        let mut eligible_pools = Vec::new();

        { // Début du scope du lock
            let graph_guard = shared_graph.lock().unwrap();
            for (address, pool) in graph_guard.pools.iter() {
                // --- NOUVEAU: Logique de double filtrage ---

                // Étape 1: Extraire les métriques nécessaires
                let (last_swap, mint_a, mint_b, reserve_a, reserve_b) = match pool {
                    // Pour les pools AMM simples, on peut vérifier les réserves
                    Pool::RaydiumAmmV4(p) => (p.last_swap_timestamp, p.mint_a, p.mint_b, p.reserve_a, p.reserve_b),
                    Pool::RaydiumCpmm(p) => (p.last_swap_timestamp, p.token_0_mint, p.token_1_mint, p.reserve_a, p.reserve_b),
                    Pool::PumpAmm(p) => (p.last_swap_timestamp, p.mint_a, p.mint_b, p.reserve_a, p.reserve_b),
                    // Pour les pools CLMM, on ne peut pas facilement avoir une "réserve", on se base donc sur l'activité seule pour l'instant
                    Pool::RaydiumClmm(p) => (p.last_swap_timestamp, p.mint_a, p.mint_b, u64::MAX, u64::MAX), // On met une valeur haute pour passer le filtre de liquidité
                    Pool::MeteoraDlmm(p) => (p.last_swap_timestamp, p.mint_a, p.mint_b, u64::MAX, u64::MAX),
                    Pool::OrcaWhirlpool(p) => (p.last_swap_timestamp, p.mint_a, p.mint_b, u64::MAX, u64::MAX),
                    _ => (0, Pubkey::default(), Pubkey::default(), 0, 0) // On ignore les autres types pour l'instant
                };

                // Filtre 1: Activité récente
                if last_swap > now - ACTIVITY_WINDOW_SECONDS {
                    // Filtre 2: Liquidité suffisante en SOL
                    /*let mut has_enough_liquidity = false;
                    if mint_a == wsol_mint && reserve_a >= MIN_SOL_LIQUIDITY {
                        has_enough_liquidity = true;
                    }
                    if mint_b == wsol_mint && reserve_b >= MIN_SOL_LIQUIDITY {
                        has_enough_liquidity = true;
                    }
                    // Pour les CLMM on a mis u64::MAX, donc ils passeront toujours
                    if reserve_a == u64::MAX && reserve_b == u64::MAX {
                        has_enough_liquidity = true;
                    }*/
                    // Filtre 2: Liquidité suffisante en SOL (TEMPORAIREMENT MODIFIÉ POUR LE TEST)
                    // On considère que la liquidité est suffisante pour tous les pools actifs pour le moment.
                    let has_enough_liquidity = true; // <-- ON FORCE LE FILTRE À PASSER

                    if has_enough_liquidity {
                        eligible_pools.push((*address, last_swap));
                    }
                }
            }
        } // Fin du scope du lock

        eligible_pools.sort_by(|a, b| b.1.cmp(&a.1));

        let active_pools: HashSet<Pubkey> = eligible_pools
            .into_iter()
            .map(|(address, _)| address)
            .collect();

        let mut hotlist = HashSet::new();
        { // Scope pour le lock
            let graph_guard = shared_graph.lock().unwrap();
            for active_pool_key in &active_pools {
                if let Some(active_pool) = graph_guard.pools.get(active_pool_key) {
                    let (active_mint_a, active_mint_b) = active_pool.get_mints();

                    // Pour chaque pool actif, on trouve tous les autres pools
                    // qui partagent la même paire de jetons.
                    for (other_pool_key, other_pool) in graph_guard.pools.iter() {
                        let (other_mint_a, other_mint_b) = other_pool.get_mints();
                        if (active_mint_a == other_mint_a && active_mint_b == other_mint_b) ||
                            (active_mint_a == other_mint_b && active_mint_b == other_mint_a)
                        {
                            // On ajoute le pool actif ET son pair à la hotlist.
                            hotlist.insert(*active_pool_key);
                            hotlist.insert(*other_pool_key);
                        }
                    }
                }
            }
        }

        // On convertit le HashSet en Vec pour la sérialisation
        let hotlist: Vec<Pubkey> = hotlist.into_iter().take(HOTLIST_SIZE).collect();

        if let Ok(json_data) = serde_json::to_string_pretty(&hotlist) {
            if fs::write("hotlist.json", json_data).is_ok() {
                println!("[Filter Engine] Hotlist mise à jour avec {} pools éligibles.", hotlist.len());
            }
        }
    }
}