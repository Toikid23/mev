use anyhow::{anyhow, Result, Context, bail}; // CORRECTION : bail importé
use futures_util::{StreamExt, sink::SinkExt};
use mev::decoders::PoolOperations;
use mev::{
    config::Config,
    decoders::Pool,
    execution::{protections, simulator, transaction_builder},
    graph_engine::Graph,
    rpc::ResilientRpcClient,
    state::slot_tracker::SlotTracker,
    strategies::spatial::{find_spatial_arbitrage, ArbitrageOpportunity},
};
use solana_address_lookup_table_program::state::AddressLookupTable;
use solana_client::nonblocking::pubsub_client::PubsubClient;
use solana_sdk::{
    message::AddressLookupTableAccount as SdkAddressLookupTableAccount, pubkey::Pubkey,
    signature::Keypair,
};
use std::{
    collections::{HashMap, HashSet},
    env,
    fs,
    io::Read,
    sync::Arc,
    time::Duration,
};
use tokio::sync::{mpsc, Mutex, RwLock};
use yellowstone_grpc_client::GeyserGrpcClient;
use yellowstone_grpc_proto::prelude::{
    subscribe_update::UpdateOneof, CommitmentLevel, SubscribeRequest,
    SubscribeRequestFilterTransactions,
};

// CORRECTION E0425 : La constante de la LUT est maintenant définie ici.
const ADDRESS_LOOKUP_TABLE_ADDRESS: Pubkey = solana_sdk::pubkey!("E5h798UBdK8V1L7MvRfi1ppr2vitPUUUUCVqvTyDgKXN");


/// Lit le fichier `hotlist.json` et retourne la liste des adresses de pools.
fn read_hotlist() -> Result<HashSet<Pubkey>> {
    let data = fs::read_to_string("hotlist.json")?;
    Ok(serde_json::from_str(&data)?)
}

/// Charge le graphe principal des pools depuis le cache binaire.
fn load_main_graph_from_cache() -> Result<Arc<Graph>> {
    println!("[Graph] Chargement du cache de pools de référence depuis 'graph_cache.bin'...");
    let file = fs::File::open("graph_cache.bin")?;
    let mut buffer = Vec::new();
    let mut reader = std::io::BufReader::new(file);
    reader.read_to_end(&mut buffer)?;
    let decoded_pools: HashMap<Pubkey, Pool> = bincode::deserialize(&buffer)?;

    let graph = Graph::new();
    {
        let mut pools_writer = graph.pools.blocking_write();
        let mut map_writer = graph.account_to_pool_map.blocking_write();
        for (key, pool) in decoded_pools.into_iter() {
            // CORRECTION E0308 : On utilise un match qui ne capture pas de variable pour éviter les conflits de type.
            match &pool {
                Pool::RaydiumClmm(_) | Pool::OrcaWhirlpool(_) | Pool::MeteoraDlmm(_) | Pool::MeteoraDammV2(_) => {
                    map_writer.insert(pool.address(), key);
                }
                _ => {
                    let (v_a, v_b) = pool.get_vaults();
                    map_writer.insert(v_a, key);
                    map_writer.insert(v_b, key);
                }
            }
            pools_writer.insert(key, Arc::new(RwLock::new(pool)));
        }
    }
    Ok(Arc::new(graph))
}


/// Le `GeyserUpdater` intelligent qui gère les deux types de pools.
struct GeyserUpdater {
    geyser_grpc_url: String,
    hot_graph: Arc<Graph>,
    update_sender: mpsc::Sender<Pubkey>,
}

impl GeyserUpdater {
    fn new(
        geyser_grpc_url: String,
        hot_graph: Arc<Graph>,
        update_sender: mpsc::Sender<Pubkey>,
    ) -> Self {
        Self { geyser_grpc_url, hot_graph, update_sender }
    }

    async fn run(&self) {
        println!("[GeyserUpdater] Démarrage du service de surveillance hybride.");
        loop {
            if let Err(e) = self.subscribe_and_process().await {
                eprintln!("[GeyserUpdater] Erreur: {}. Reconnexion dans 5s...", e);
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        }
    }

    async fn subscribe_and_process(&self) -> Result<()> {
        let mut client = GeyserGrpcClient::build_from_shared(self.geyser_grpc_url.clone())?
            .connect().await.context("Connexion Geyser gRPC échouée")?;
        let (mut subscribe_tx, mut stream) = client.subscribe().await?;

        let mut watch_to_pool_map: HashMap<String, Pubkey> = HashMap::new();

        let mut interval = tokio::time::interval(Duration::from_secs(15));
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    let hotlist_pools = read_hotlist().unwrap_or_default();
                    let mut accounts_to_watch = HashSet::new();
                    watch_to_pool_map.clear();

                    let graph_pools_reader = self.hot_graph.pools.read().await;
                    for pool_addr in &hotlist_pools {
                        if let Some(pool_arc) = graph_pools_reader.get(pool_addr) {
                            let pool = pool_arc.read().await;

                            // CORRECTION E0308 : On utilise le même `match` simple ici.
                            match &*pool {
                                Pool::RaydiumClmm(_) | Pool::OrcaWhirlpool(_) | Pool::MeteoraDlmm(_) | Pool::MeteoraDammV2(_) => {
                                    let addr_str = pool.address().to_string();
                                    accounts_to_watch.insert(addr_str.clone());
                                    watch_to_pool_map.insert(addr_str, pool.address());
                                },
                                _ => {
                                    let (v_a, v_b) = pool.get_vaults();
                                    let v_a_str = v_a.to_string();
                                    let v_b_str = v_b.to_string();
                                    accounts_to_watch.insert(v_a_str.clone());
                                    accounts_to_watch.insert(v_b_str.clone());
                                    watch_to_pool_map.insert(v_a_str, pool.address());
                                    watch_to_pool_map.insert(v_b_str, pool.address());
                                }
                            }
                        }
                    }

                    if !accounts_to_watch.is_empty() {
                        println!("[GeyserUpdater] Mise à jour de l'abonnement pour {} comptes.", accounts_to_watch.len());
                        let mut tx_filter = HashMap::new();
                        tx_filter.insert(
                            "txs".to_string(),
                            SubscribeRequestFilterTransactions {
                                vote: Some(false), failed: Some(false),
                                account_include: accounts_to_watch.into_iter().collect(),
                                account_required: vec![], account_exclude: vec![], signature: None,
                            },
                        );
                        let request = SubscribeRequest {
                            transactions: tx_filter,
                            commitment: Some(CommitmentLevel::Processed as i32),
                            ..Default::default()
                        };
                        if subscribe_tx.send(request).await.is_err() {
                            bail!("Le canal d'abonnement Geyser est fermé.");
                        }
                    }
                }

                message_result = stream.next() => {
                    let message = match message_result {
                        Some(res) => res?,
                        None => break,
                    };
                    if let Some(UpdateOneof::Transaction(tx_update)) = message.update_oneof {
                        if let Some(tx_info) = tx_update.transaction {
                            if let Some(tx) = tx_info.transaction {
                                if let Some(msg) = tx.message {
                                    let mut affected_pools = HashSet::new();
                                    for key_bytes in &msg.account_keys {
                                        if key_bytes.len() == 32 {
                                            let key_str = Pubkey::new_from_array(key_bytes.as_slice().try_into()?).to_string();
                                            if let Some(pool_addr) = watch_to_pool_map.get(&key_str) {
                                                affected_pools.insert(*pool_addr);
                                            }
                                        }
                                    }
                                    for pool_addr in affected_pools {
                                        let _ = self.update_sender.send(pool_addr).await;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        Err(anyhow!("Stream Geyser terminé."))
    }
}


/// Déclenche la ré-hydratation d'un pool et cherche des opportunités.
async fn rehydrate_and_find_opportunities(
    pool_address: Pubkey,
    graph: Arc<Graph>,
    rpc_client: Arc<ResilientRpcClient>,
    payer: Keypair,
    currently_processing: Arc<Mutex<HashSet<String>>>,
    slot_tracker: Arc<SlotTracker>,
) {
    let pool_arc = {
        let pools_reader = graph.pools.read().await;
        pools_reader.get(&pool_address).cloned()
    };

    if let Some(pool_arc) = pool_arc {
        let mut pool_writer = pool_arc.write().await;
        let pool_type = pool_writer.clone();

        let rehydrate_result = match pool_type {
            Pool::RaydiumAmmV4(mut p) => mev::decoders::raydium::amm_v4::hydrate(&mut p, &rpc_client).await.map(|_| Pool::RaydiumAmmV4(p)),
            Pool::RaydiumCpmm(mut p) => mev::decoders::raydium::cpmm::hydrate(&mut p, &rpc_client).await.map(|_| Pool::RaydiumCpmm(p)),
            Pool::RaydiumClmm(mut p) => mev::decoders::raydium::clmm::hydrate(&mut p, &rpc_client).await.map(|_| Pool::RaydiumClmm(p)),
            Pool::MeteoraDammV1(mut p) => mev::decoders::meteora::damm_v1::hydrate(&mut p, &rpc_client).await.map(|_| Pool::MeteoraDammV1(p)),
            Pool::MeteoraDammV2(mut p) => mev::decoders::meteora::damm_v2::hydrate(&mut p, &rpc_client).await.map(|_| Pool::MeteoraDammV2(p)),
            Pool::MeteoraDlmm(mut p) => mev::decoders::meteora::dlmm::hydrate(&mut p, &rpc_client).await.map(|_| Pool::MeteoraDlmm(p)),
            Pool::OrcaWhirlpool(mut p) => mev::decoders::orca::whirlpool::hydrate(&mut p, &rpc_client).await.map(|_| Pool::OrcaWhirlpool(p)),
            Pool::PumpAmm(mut p) => mev::decoders::pump::amm::hydrate(&mut p, &rpc_client).await.map(|_| Pool::PumpAmm(p)),
        };

        if let Ok(hydrated_pool) = rehydrate_result {
            *pool_writer = hydrated_pool;
        } else {
            return;
        }
        drop(pool_writer); // On relâche le verrou en écriture le plus tôt possible

        let clock_snapshot = slot_tracker.current();
        let current_timestamp = clock_snapshot.unix_timestamp;
        let opportunities = find_spatial_arbitrage(graph.clone()).await;

        if let Some(opp) = opportunities.into_iter().next() {
            let mut pools = [opp.pool_buy_from_key.to_string(), opp.pool_sell_to_key.to_string()];
            pools.sort();
            let opportunity_id = format!("{}-{}", pools[0], pools[1]);

            let is_already_processing = {
                let mut processing_guard = currently_processing.lock().await;
                if processing_guard.contains(&opportunity_id) { true }
                else { processing_guard.insert(opportunity_id.clone()); false }
            };

            if !is_already_processing {
                if let Err(e) = process_opportunity(opp, graph, rpc_client, payer, current_timestamp).await {
                    println!("[Erreur Traitement] {}", e);
                }
                currently_processing.lock().await.remove(&opportunity_id);
            }
        }
    }
}

async fn process_opportunity( opportunity: ArbitrageOpportunity, graph: Arc<Graph>, rpc_client: Arc<ResilientRpcClient>, payer: Keypair, current_timestamp: i64) -> Result<()> {
    let lookup_table_account_data = rpc_client.get_account_data(&ADDRESS_LOOKUP_TABLE_ADDRESS).await?;
    let lookup_table_ref = AddressLookupTable::deserialize(&lookup_table_account_data)?;
    let owned_lookup_table = SdkAddressLookupTableAccount {
        key: ADDRESS_LOOKUP_TABLE_ADDRESS,
        addresses: lookup_table_ref.addresses.to_vec(),
    };
    let protections = {
        let pool_sell_to = {
            let pools_reader = graph.pools.read().await;
            let pool_arc_rwlock = pools_reader.get(&opportunity.pool_sell_to_key).context("Pool not in graph")?.clone();
            drop(pools_reader);
            let pool_guard = pool_arc_rwlock.read().await;
            (*pool_guard).clone()
        };
        match protections::calculate_slippage_protections(
            opportunity.amount_in,
            opportunity.profit_in_lamports,
            pool_sell_to,
            &opportunity.token_intermediate_mint,
            current_timestamp,
        ) {
            Ok(p) => p,
            Err(e) => { println!("[Phase 1 ERREUR] Échec calcul protections : {}", e); return Ok(()) }
        }
    };
    let (final_arbitrage_tx, _final_execute_route_ix) = match transaction_builder::build_arbitrage_transaction(
        &opportunity, graph.clone(), &rpc_client, &payer, &owned_lookup_table, Some(&protections),
    ).await {
        Ok(res) => res,
        Err(e) => { println!("[Phase 2 ERREUR] Échec construction tx finale : {}", e); return Ok(()) }
    };
    let accounts_for_fees = vec![opportunity.pool_buy_from_key, opportunity.pool_sell_to_key];
    let sim_data = match simulator::run_simulations(rpc_client.clone(), &final_arbitrage_tx, accounts_for_fees).await {
        Ok(data) => data,
        Err(e) => { println!("[Phase 3 VALIDATION ÉCHOUÉE] La transaction n'est plus viable : {}", e); return Ok(()) }
    };
    println!("  -> Validation et Découverte RÉUSSIES !");
    let profit_brut_reel = sim_data.profit_brut_reel;
    let compute_units = sim_data.compute_units;
    let priority_fees = sim_data.priority_fees;
    println!("\n--- [Phase 4] Calculs finaux et décision d'envoi ---");
    let mut recent_fees: Vec<u64> = priority_fees.iter().map(|f| f.prioritization_fee).collect();
    recent_fees.sort_unstable();
    let percentile_index = (recent_fees.len() as f64 * 0.8).floor() as usize;
    let dynamic_priority_fee_price_per_cu = *recent_fees.get(percentile_index).unwrap_or(&1000);
    let total_frais_normaux = 5000 + (compute_units * dynamic_priority_fee_price_per_cu) / 1_000_000;
    let profit_net_final = profit_brut_reel.saturating_sub(total_frais_normaux);
    println!("     -> Profit Net FINAL Calculé : {} lamports", profit_net_final);
    if profit_net_final > 0 {
        println!("  -> DÉCISION: ENVOYER LA TRANSACTION NORMALE");
    } else {
        println!("  -> DÉCISION : Abandon. Le profit réel est insuffisant après frais.");
    }
    println!("\n--- [Phase 5] Envoi (Mode Simulation) ---");
    println!("[ACTION SIMULÉE - TX NORMALE]");
    println!("  -> Profit Net Attendu: {} lamports", profit_net_final);
    println!("  -> Enverrait une transaction avec un priority fee de {} micro-lamports/CU.", dynamic_priority_fee_price_per_cu);
    Ok(())
}


#[tokio::main]
async fn main() -> Result<()> {
    println!("--- Lancement de l'Execution Bot (Avec Geyser gRPC Hybride) ---");
    dotenvy::dotenv().ok();

    let config = Config::load()?;
    let payer = Keypair::from_base58_string(&config.payer_private_key);
    let rpc_client = Arc::new(ResilientRpcClient::new(config.solana_rpc_url.clone(), 3, 500));
    let geyser_url = env::var("GEYSER_GRPC_URL").context("GEYSER_GRPC_URL doit être défini dans le fichier .env")?;

    let main_graph = load_main_graph_from_cache()?;
    let hot_graph = Arc::new(Graph::new());

    let (update_sender, mut update_receiver) = mpsc::channel::<Pubkey>(1024);

    let slot_tracker = Arc::new(SlotTracker::new(&rpc_client).await?);
    let wss_url = config.solana_rpc_url.replace("http", "ws");
    let pubsub_client = Arc::new(PubsubClient::new(&wss_url).await?);
    slot_tracker.start(rpc_client.clone(), pubsub_client);

    let geyser_updater = GeyserUpdater::new(geyser_url, hot_graph.clone(), update_sender.clone());
    tokio::spawn(async move {
        geyser_updater.run().await;
    });

    let currently_processing = Arc::new(Mutex::new(HashSet::<String>::new()));

    let hot_graph_clone_for_onboarding = Arc::clone(&hot_graph);
    let main_graph_clone_for_onboarding = Arc::clone(&main_graph);
    let rpc_client_clone_for_onboarding = Arc::clone(&rpc_client);
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(5));
        loop {
            interval.tick().await;
            if let Ok(hotlist) = read_hotlist() {
                let mut pools_to_add = Vec::new();
                {
                    let graph_pools_reader = hot_graph_clone_for_onboarding.pools.read().await;
                    for pool_address in hotlist {
                        if !graph_pools_reader.contains_key(&pool_address) {
                            pools_to_add.push(pool_address);
                        }
                    }
                }
                for pool_address in pools_to_add {
                    let unhydrated_pool = {
                        let main_pools_reader = main_graph_clone_for_onboarding.pools.read().await;
                        let arc_rwlock = match main_pools_reader.get(&pool_address) { Some(p) => p.clone(), None => continue };
                        drop(main_pools_reader);
                        let pool_guard = arc_rwlock.read().await;
                        (*pool_guard).clone()
                    };
                    if let Ok(hydrated_pool) = hot_graph_clone_for_onboarding.hydrate_pool(unhydrated_pool, &rpc_client_clone_for_onboarding).await {
                        hot_graph_clone_for_onboarding.add_pool_to_graph(hydrated_pool).await;
                        println!("[Admission] Nouveau pool {} ajouté au hot graph.", pool_address);
                    }
                }
            }
        }
    });

    println!("[Bot] Prêt. En attente des mises à jour de Geyser...");
    loop {
        if let Some(pool_to_update) = update_receiver.recv().await {
            let graph_clone = Arc::clone(&hot_graph);
            let rpc_clone = Arc::clone(&rpc_client);
            let payer_clone = Keypair::try_from(payer.to_bytes().as_slice())?;
            let processing_clone = Arc::clone(&currently_processing);
            let tracker_clone = Arc::clone(&slot_tracker);

            tokio::spawn(async move {
                rehydrate_and_find_opportunities(
                    pool_to_update,
                    graph_clone,
                    rpc_clone,
                    payer_clone,
                    processing_clone,
                    tracker_clone,
                ).await;
            });
        }
    }
}