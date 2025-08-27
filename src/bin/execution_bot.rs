use anyhow::{anyhow, Result}; // anyhow est nécessaire pour les .ok_or_else()
use futures_util::StreamExt;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use mev::{
    config::Config,
    decoders::{Pool, PoolOperations},
    graph_engine::Graph,
    strategies::spatial::{find_spatial_arbitrage, ArbitrageOpportunity}, // L'opportunité elle-même
};
use solana_client::{
    nonblocking::{pubsub_client::PubsubClient, rpc_client::RpcClient},
    rpc_config::RpcSimulateTransactionConfig, // Pour la simulation
};
use solana_account_decoder::{UiAccountEncoding, UiAccountData};
use solana_program_pack::Pack;
use solana_sdk::{
    compute_budget::ComputeBudgetInstruction, // Pour les frais de prio
    instruction::Instruction,
    pubkey::Pubkey,
    signature::Keypair, // Pour le portefeuille
    signer::Signer,
    transaction::Transaction, // Pour construire la transaction
};
use spl_associated_token_account::get_associated_token_address_with_program_id;
use std::{collections::{HashMap, HashSet}, fs, io::Read, str::FromStr, sync::Arc};
use tokio::{sync::{mpsc, Mutex}, task::JoinHandle, time};
use solana_client::rpc_config::RpcAccountInfoConfig;
use spl_token::state::Account as SplTokenAccount;
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

async fn subscribe_to_hot_vault(
    pubsub_client: Arc<PubsubClient>,
    vault_address: Pubkey,
    update_sender: mpsc::Sender<(Pubkey, u64)>,
) {
    let config = RpcAccountInfoConfig { encoding: Some(UiAccountEncoding::Base64), ..Default::default() };
    let (mut stream, _unsubscribe) = pubsub_client.account_subscribe(&vault_address, Some(config)).await.unwrap();
    while let Some(response) = stream.next().await {
        let ui_account = response.value;
        let data = match ui_account.data {
            UiAccountData::Binary(encoded_data, _) => STANDARD.decode(encoded_data).unwrap_or_default(),
            _ => continue,
        };
        if let Ok(token_account) = SplTokenAccount::unpack(&data) {
            if update_sender.send((vault_address, token_account.amount)).await.is_err() {
                break;
            }
        }
    }
}

fn update_hot_graph_reserve(graph: &Arc<Mutex<Graph>>, vault_address: &Pubkey, new_balance: u64) {
    // On utilise `try_lock` pour ne pas bloquer si le mutex est déjà pris par la tâche d'arbitrage.
    if let Ok(mut graph_guard) = graph.try_lock() {
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
                    _ => {}
                }
            }
        }
    }
}


async fn build_and_simulate_arbitrage_tx(
    opportunity: &ArbitrageOpportunity,
    graph: Arc<Mutex<Graph>>,
    rpc_client: Arc<RpcClient>,
    payer: &Keypair,
) -> Result<()> {
    println!("\n--- Simulation d'une Opportunité ---");
    println!("{:?}", opportunity);

    let mut graph_guard = graph.lock().await;
    let pool_buy_from = graph_guard.pools.get_mut(&opportunity.pool_buy_from_key).ok_or_else(|| anyhow!("Pool A not found"))?;
    let pool_sell_to = graph_guard.pools.get_mut(&opportunity.pool_sell_to_key).ok_or_else(|| anyhow!("Pool B not found"))?;

    // On a besoin des programmes de token pour les ATAs
    // Cette partie devra être améliorée pour extraire dynamiquement les token_programs des pools
    let token_in_program = spl_token::id();
    let token_intermediate_program = spl_token::id();

    let user_ata_in = get_associated_token_address_with_program_id(&payer.pubkey(), &opportunity.token_in_mint, &token_in_program);
    let user_ata_intermediate = get_associated_token_address_with_program_id(&payer.pubkey(), &opportunity.token_intermediate_mint, &token_intermediate_program);

    let mut instructions = vec![
        ComputeBudgetInstruction::set_compute_unit_limit(1_000_000),
        ComputeBudgetInstruction::set_compute_unit_price(100_000), // Priorisation
    ];

    // Instruction d'achat
    // TODO: Il faut une méthode unifiée pour créer des instructions de swap sur l'enum Pool
    // let ix_buy = pool_buy_from.create_swap_instruction(...);
    // instructions.push(ix_buy);

    // Instruction de vente
    // TODO:
    // let ix_sell = pool_sell_to.create_swap_instruction(...);
    // instructions.push(ix_sell);

    if instructions.len() <= 2 {
        println!("[Simulation] Pas d'instructions de swap à simuler (TODO).");
        return Ok(());
    }

    let recent_blockhash = rpc_client.get_latest_blockhash().await?;
    let tx = Transaction::new_signed_with_payer(&instructions, Some(&payer.pubkey()), &[payer], recent_blockhash);

    let sim_config = RpcSimulateTransactionConfig { sig_verify: false, replace_recent_blockhash: true, ..Default::default() };
    let sim_result = rpc_client.simulate_transaction_with_config(&tx, sim_config).await?;

    if let Some(err) = sim_result.value.err {
        println!("[Simulation] ÉCHEC : {:?}", err);
    } else {
        println!("[Simulation] SUCCÈS ! Profit potentiel validé.");
        // TODO: Analyser les changements de balance pour calculer le profit simulé net.
    }

    Ok(())
}



#[tokio::main]
async fn main() -> Result<()> {
    println!("--- Lancement de l'Execution Bot (Version de Développement) ---");

    let config = Config::load()?;
    let payer = Keypair::from_base58_string(&config.payer_private_key);
    let rpc_client = Arc::new(RpcClient::new(config.solana_rpc_url.clone()));
    let main_graph = Arc::new(load_main_graph_from_cache()?);
    let hot_graph = Arc::new(Mutex::new(Graph::new())); // CORRECTION: Utilise tokio::sync::Mutex
    let wss_url = config.solana_rpc_url.replace("http", "ws");
    let pubsub_client = Arc::new(PubsubClient::new(&wss_url).await?);
    let (update_sender, mut update_receiver) = mpsc::channel::<(Pubkey, u64)>(100);

    let hot_graph_clone_for_task1 = Arc::clone(&hot_graph);
    let main_graph_clone_for_task1 = Arc::clone(&main_graph);
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
                    *hot_graph_clone_for_task1.lock().await = new_hot_graph; // .await est nécessaire pour le Mutex de Tokio
                    current_subscribed_vaults = target_vaults;
                    println!("[Hotlist] Abonnements actifs: {}", current_subscribed_vaults.len());
                }
            }
            time::sleep(time::Duration::from_secs(5)).await;
        }
    });

    println!("[Bot] Prêt. En attente d'opportunités d'arbitrage...");
    loop {
        if let Some((vault_address, new_balance)) = update_receiver.recv().await {
            update_hot_graph_reserve(&hot_graph, &vault_address, new_balance);

            let graph_clone = Arc::clone(&hot_graph);
            let client_clone = Arc::clone(&rpc_client);
            let payer_clone = Keypair::from_bytes(&payer.to_bytes())?; // Cloner le payeur

            tokio::spawn(async move {
                let opportunities = find_spatial_arbitrage(graph_clone.clone(), client_clone.clone()).await;
                for opp in opportunities {
                    if let Err(e) = build_and_simulate_arbitrage_tx(&opp, graph_clone.clone(), client_clone.clone(), &payer_clone).await {
                        println!("[Erreur Simulation] {}", e);
                    }
                }
            });
        }
    }
}