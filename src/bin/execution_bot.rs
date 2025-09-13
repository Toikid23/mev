// DANS : src/bin/execution_bot.rs (Version Définitive)

use anyhow::{anyhow, Result, Context, bail};
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
    signature::Keypair, signer::Signer, transaction::Transaction,
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
    SubscribeRequestFilterAccounts, // <-- Le bon import
};
use spl_associated_token_account::instruction::create_associated_token_account;
use solana_sdk::transaction::VersionedTransaction;


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
            let (v_a, v_b) = pool.get_vaults();
            map_writer.insert(v_a, key);
            map_writer.insert(v_b, key);
            map_writer.insert(pool.address(), key); // Pour les CLMM
            pools_writer.insert(key, Arc::new(RwLock::new(pool)));
        }
    }
    Ok(Arc::new(graph))
}

/// Vérifie si le compte de volume pump.fun de l'utilisateur existe, et le crée si ce n'est pas le cas.
async fn ensure_pump_user_account_exists(rpc_client: &Arc<ResilientRpcClient>, payer: &Keypair) -> Result<()> {
    println!("\n[Pré-vérification] Vérification du compte de volume utilisateur pump.fun...");
    let (pda, _) = Pubkey::find_program_address(&[b"user_volume_accumulator", payer.pubkey().as_ref()], &mev::decoders::pump::amm::PUMP_PROGRAM_ID);
    if rpc_client.get_account(&pda).await.is_err() {
        println!("  -> Compte de volume non trouvé. Création en cours...");
        let init_ix = mev::decoders::pump::amm::pool::create_init_user_volume_accumulator_instruction(&payer.pubkey())?;
        let recent_blockhash = rpc_client.get_latest_blockhash().await?;
        let transaction = Transaction::new_signed_with_payer(&[init_ix], Some(&payer.pubkey()), &[payer], recent_blockhash);
        let versioned_tx = VersionedTransaction::from(transaction);
        let signature = rpc_client.send_and_confirm_transaction(&versioned_tx).await?;
        println!("  -> ✅ SUCCÈS ! Compte de volume créé. Signature : {}", signature);
    } else {
        println!("  -> Compte de volume déjà existant.");
    }
    Ok(())
}

/// Vérifie l'existence des ATAs pour les deux mints d'un pool et les crée si nécessaire.
async fn ensure_atas_exist_for_pool(rpc_client: &Arc<ResilientRpcClient>, payer: &Keypair, pool: &Pool) -> Result<()> {
    let (mint_a, mint_b) = pool.get_mints();
    let (mint_a_program, mint_b_program) = match pool {
        Pool::RaydiumClmm(p) => (p.mint_a_program, p.mint_b_program),
        Pool::OrcaWhirlpool(p) => (p.mint_a_program, p.mint_b_program),
        Pool::MeteoraDlmm(p) => (p.mint_a_program, p.mint_b_program),
        Pool::MeteoraDammV2(p) => (p.mint_a_program, p.mint_b_program),
        Pool::RaydiumCpmm(p) => (p.token_0_program, p.token_1_program),
        Pool::PumpAmm(p) => (p.mint_a_program, p.mint_b_program),
        _ => (spl_token::id(), spl_token::id()),
    };
    let ata_a_address = spl_associated_token_account::get_associated_token_address_with_program_id(&payer.pubkey(), &mint_a, &mint_a_program);
    let ata_b_address = spl_associated_token_account::get_associated_token_address_with_program_id(&payer.pubkey(), &mint_b, &mint_b_program);
    let accounts_to_check = vec![ata_a_address, ata_b_address];
    let results = rpc_client.get_multiple_accounts(&accounts_to_check).await?;
    let mut instructions_to_execute = Vec::new();
    if results[0].is_none() {
        instructions_to_execute.push(create_associated_token_account(&payer.pubkey(), &payer.pubkey(), &mint_a, &mint_a_program));
    }
    if results[1].is_none() {
        instructions_to_execute.push(create_associated_token_account(&payer.pubkey(), &payer.pubkey(), &mint_b, &mint_b_program));
    }
    if !instructions_to_execute.is_empty() {
        println!("[Admission] Envoi de la transaction pour créer {} ATA(s)...", instructions_to_execute.len());
        let recent_blockhash = rpc_client.get_latest_blockhash().await?;
        let transaction = Transaction::new_signed_with_payer(&instructions_to_execute, Some(&payer.pubkey()), &[payer], recent_blockhash);
        let versioned_tx = VersionedTransaction::from(transaction);
        let signature = rpc_client.send_and_confirm_transaction(&versioned_tx).await?;
        println!("[Admission] ✅ ATA(s) créé(s) avec succès. Signature : {}", signature);
    }
    Ok(())
}

/// Le GeyserUpdater qui utilise `subscribeAccountUpdates`
struct GeyserUpdater {
    geyser_grpc_url: String,
    hot_graph: Arc<Graph>,
    update_sender: mpsc::Sender<Pubkey>,
}

impl GeyserUpdater {
    fn new(geyser_grpc_url: String, hot_graph: Arc<Graph>, update_sender: mpsc::Sender<Pubkey>) -> Self {
        Self { geyser_grpc_url, hot_graph, update_sender }
    }

    async fn run(&self) {
        println!("[GeyserUpdater] Démarrage du service de surveillance ciblé (AccountUpdates).");
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
        let mut watch_to_pool_map: HashMap<Pubkey, Pubkey> = HashMap::new();
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
                            match &*pool {
                                Pool::RaydiumClmm(_) | Pool::OrcaWhirlpool(_) | Pool::MeteoraDlmm(_) | Pool::MeteoraDammV2(_) => {
                                    accounts_to_watch.insert(pool.address());
                                    watch_to_pool_map.insert(pool.address(), pool.address());
                                },
                                _ => {
                                    let (v_a, v_b) = pool.get_vaults();
                                    accounts_to_watch.insert(v_a);
                                    accounts_to_watch.insert(v_b);
                                    watch_to_pool_map.insert(v_a, pool.address());
                                    watch_to_pool_map.insert(v_b, pool.address());
                                }
                            }
                        }
                    }
                    if !accounts_to_watch.is_empty() {
                        println!("[GeyserUpdater] Mise à jour de l'abonnement pour {} comptes.", accounts_to_watch.len());
                        let mut accounts_filter = HashMap::new();

                        // --- LA CORRECTION DÉFINITIVE BASÉE SUR LA SOURCE ---
                        accounts_filter.insert(
                            "accounts".to_string(),
                            SubscribeRequestFilterAccounts {
                                // On remplit le champ `account` avec notre liste de pubkeys
                                account: accounts_to_watch.into_iter().map(|p| p.to_string()).collect(),
                                // `owner` peut rester vide
                                owner: vec![],
                                // `filters` (pour memcmp) peut rester vide
                                filters: vec![],
                                // `nonempty_txn_signature` est optionnel, `None` est une valeur sûre.
                                nonempty_txn_signature: None,
                            },
                        );

                        let request = SubscribeRequest {
                            accounts: accounts_filter,
                            commitment: Some(CommitmentLevel::Processed as i32),
                            ..Default::default()
                        };
                        if subscribe_tx.send(request).await.is_err() {
                            bail!("Le canal d'abonnement Geyser est fermé.");
                        }
                    }
                }
                message_result = stream.next() => {
                    let message = match message_result { Some(res) => res?, None => break };
                    if let Some(UpdateOneof::Account(account_update)) = message.update_oneof {
                        // --- CORRECTION DE L'ACCÈS AUX DONNÉES ---
                        let rpc_account = account_update.account.context("Le message Geyser ne contenait pas de données de compte")?;
                        let account_key = Pubkey::try_from(rpc_account.pubkey).map_err(|e| anyhow!("Impossible de convertir la pubkey: {:?}", e))?;

                        if let Some(pool_addr) = watch_to_pool_map.get(&account_key) {
                            let pool_arc = {
                                let reader = self.hot_graph.pools.read().await;
                                reader.get(pool_addr).cloned()
                            };
                            if let Some(pool_arc) = pool_arc {
                                let mut pool_writer = pool_arc.write().await;
                                if let Err(e) = pool_writer.update_from_account_data(&account_key, &rpc_account.data) {
                                     eprintln!("[GeyserUpdater] Erreur de mise à jour pour {}: {}", pool_addr, e);
                                     continue;
                                }
                                drop(pool_writer);
                                let _ = self.update_sender.send(*pool_addr).await;
                            }
                        }
                    }
                }
            }
        }
        Err(anyhow!("Stream Geyser terminé."))
    }
}

/// Logique de ré-hydratation simplifiée
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
        let pool_type_clone = (*pool_writer).clone();

        let rehydrate_result = match pool_type_clone {
            Pool::RaydiumClmm(mut p) => mev::decoders::raydium::clmm::hydrate(&mut p, &rpc_client).await.map(|_| Pool::RaydiumClmm(p)),
            Pool::OrcaWhirlpool(mut p) => mev::decoders::orca::whirlpool::hydrate(&mut p, &rpc_client).await.map(|_| Pool::OrcaWhirlpool(p)),
            Pool::MeteoraDlmm(mut p) => mev::decoders::meteora::dlmm::hydrate(&mut p, &rpc_client).await.map(|_| Pool::MeteoraDlmm(p)),
            _ => Ok(pool_type_clone),
        };

        if let Ok(hydrated_pool) = rehydrate_result {
            *pool_writer = hydrated_pool;
        } else {
            eprintln!("[Rehydrate] Échec de la ré-hydratation des dépendances pour {}", pool_address);
            return;
        }

        drop(pool_writer);

        let clock_snapshot = slot_tracker.current();
        let opportunities = find_spatial_arbitrage(graph.clone()).await;
        if let Some(opp) = opportunities.into_iter().next() {
            let mut pools = [opp.pool_buy_from_key.to_string(), opp.pool_sell_to_key.to_string()];
            pools.sort();
            let opportunity_id = format!("{}-{}", pools[0], pools[1]);
            let is_already_processing = {
                let mut processing_guard = currently_processing.lock().await;
                if processing_guard.contains(&opportunity_id) { true } else { processing_guard.insert(opportunity_id.clone()); false }
            };
            if !is_already_processing {
                // --- CORRECTION DE L'APPEL AVEC TIMESTAMP ---
                if let Err(e) = process_opportunity(opp, graph, rpc_client, payer, clock_snapshot.unix_timestamp).await {
                    println!("[Erreur Traitement] {}", e);
                }
                currently_processing.lock().await.remove(&opportunity_id);
            }
        }
    }
}

/// La fonction `process_opportunity` reste la même
async fn process_opportunity( opportunity: ArbitrageOpportunity, graph: Arc<Graph>, rpc_client: Arc<ResilientRpcClient>, payer: Keypair, current_timestamp: i64) -> Result<()> {
    // ... (aucun changement ici)
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
    println!("--- Lancement de l'Execution Bot (Architecture Finale) ---");
    dotenvy::dotenv().ok();

    let config = Config::load()?;
    let payer = Keypair::from_base58_string(&config.payer_private_key);
    let rpc_client = Arc::new(ResilientRpcClient::new(config.solana_rpc_url.clone(), 3, 500));
    let geyser_url = env::var("GEYSER_GRPC_URL").context("GEYSER_GRPC_URL doit être défini dans le fichier .env")?;

    ensure_pump_user_account_exists(&rpc_client, &payer).await?;
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
    let payer_clone_for_onboarding = Keypair::try_from(payer.to_bytes().as_slice())?;

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
                        if let Err(e) = ensure_atas_exist_for_pool(&rpc_client_clone_for_onboarding, &payer_clone_for_onboarding, &hydrated_pool).await {
                            eprintln!("[Admission ERREUR] Impossible de créer les ATAs pour le pool {}: {}. On ignore ce pool pour le moment.", pool_address, e);
                            continue;
                        }
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