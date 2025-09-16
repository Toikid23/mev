use anyhow::{anyhow, Result, Context, bail};
use arc_swap::ArcSwap;
use futures_util::{StreamExt, sink::SinkExt};
use mev::decoders::PoolOperations;
use solana_client::rpc_config::RpcSimulateTransactionConfig;
use solana_transaction_status::UiTransactionEncoding;
use mev::execution::cu_manager;
use std::str::FromStr;
use mev::{
    config::Config,
    decoders::Pool,
    execution::{fee_manager::FeeManager, protections, simulator, transaction_builder},
    graph_engine::Graph,
    rpc::ResilientRpcClient,
    state::{
        leader_schedule::LeaderScheduleTracker,
        slot_metronome::SlotMetronome,
        slot_tracker::SlotTracker,
        validator_intel::ValidatorIntelService,
    },
    strategies::spatial::{find_spatial_arbitrage, ArbitrageOpportunity},
};
use chrono::Utc;
use solana_address_lookup_table_program::state::AddressLookupTable;
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
    time::{Duration, Instant},
};
use tokio::sync::{Mutex, watch}; // <-- NOUVEL IMPORT
use yellowstone_grpc_client::GeyserGrpcClient;
use yellowstone_grpc_proto::prelude::{
    subscribe_update::UpdateOneof, CommitmentLevel, SubscribeRequest,
    SubscribeRequestFilterAccounts,
};
use spl_associated_token_account::instruction::create_associated_token_account;
use solana_sdk::transaction::VersionedTransaction;
use tracing::{info, warn, error, instrument};
use mev::monitoring::metrics;
use tokio::sync::mpsc;


const MANAGED_LUT_ADDRESS: &str = "E5h798UBdK8V1L7MvRfi1ppr2vitPUUUUCVqvTyDgKXN";

const BOT_PROCESSING_TIME_MS: u128 = 200;
const JITO_TIP_PERCENT: u64 = 20;
const ANALYSIS_WORKER_COUNT: usize = 4; // <-- NOUVEAU : Nombre de workers d'analyse

lazy_static::lazy_static! {
    static ref JITO_REGIONAL_ENDPOINTS: HashMap<String, String> = {
        let mut map = HashMap::new();
        map.insert("Amsterdam".to_string(), "https://amsterdam.mainnet.block-engine.jito.wtf".to_string());
        map.insert("Dublin".to_string(), "https://dublin.mainnet.block-engine.jito.wtf".to_string());
        map.insert("Frankfurt".to_string(), "https://frankfurt.mainnet.block-engine.jito.wtf".to_string());
        map.insert("London".to_string(), "https://london.mainnet.block-engine.jito.wtf".to_string());
        map.insert("New York".to_string(), "https://ny.mainnet.block-engine.jito.wtf".to_string());
        map.insert("Salt Lake City".to_string(), "https://slc.mainnet.block-engine.jito.wtf".to_string());
        map.insert("Singapore".to_string(), "https://singapore.mainnet.block-engine.jito.wtf".to_string());
        map.insert("Tokyo".to_string(), "https://tokyo.mainnet.block-engine.jito.wtf".to_string());
        map
    };
}

// Les fonctions helpers (read_hotlist, load_main_graph_from_cache, etc.) restent les mêmes.
// ... (copiez-collez ici les fonctions read_hotlist, load_main_graph_from_cache, ensure_pump_user_account_exists, ensure_atas_exist_for_pool)
// NOTE : `load_main_graph_from_cache` doit maintenant retourner `Arc<Graph>` où `Graph` n'a plus de verrous internes.
fn read_hotlist() -> Result<HashSet<Pubkey>> {
    let data = fs::read_to_string("hotlist.json")?;
    Ok(serde_json::from_str(&data)?)
}
fn load_main_graph_from_cache() -> Result<Graph> { // <-- MODIFIÉ : retourne Graph, pas Arc<Graph>
    println!("[Graph] Chargement du cache de pools de référence depuis 'graph_cache.bin'...");
    let file = fs::File::open("graph_cache.bin")?;
    let mut buffer = Vec::new();
    let mut reader = std::io::BufReader::new(file);
    reader.read_to_end(&mut buffer)?;
    let decoded_pools: HashMap<Pubkey, Pool> = bincode::deserialize(&buffer)?;

    let mut graph = Graph::new();
    for (_, pool) in decoded_pools.into_iter() {
        graph.add_pool_to_graph(pool); // Utilise la nouvelle méthode
    }
    Ok(graph)
}
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


// --- NOUVELLE ARCHITECTURE : LE PRODUCTEUR DE DONNÉES ---
struct DataProducer {
    geyser_grpc_url: String,
    shared_graph: Arc<ArcSwap<Graph>>,
    main_graph: Arc<Graph>, // Le graphe de référence, en lecture seule
    shared_hotlist: Arc<tokio::sync::RwLock<HashSet<Pubkey>>>,
    update_notifier: watch::Sender<()>,
    rpc_client: Arc<ResilientRpcClient>,
    payer: Keypair,
}

impl DataProducer {
    async fn run(&self) {
        println!("[Producer] Démarrage du service de mise à jour du graphe.");
        // On fusionne la logique de l'Onboarding Manager et du GeyserUpdater ici.
        let geyser_task = self.run_geyser_listener();
        let onboarding_task = self.run_onboarding_manager();

        // On exécute les deux tâches en parallèle.
        tokio::join!(geyser_task, onboarding_task);
    }

    // Tâche pour intégrer les nouveaux pools de la hotlist
    async fn run_onboarding_manager(&self) {
        let mut processed_pools = HashSet::new();
        let mut interval = tokio::time::interval(Duration::from_secs(5));
        loop {
            interval.tick().await;
            let hotlist_snapshot = self.shared_hotlist.read().await.clone();
            let mut graph_changed = false;

            // On charge le graphe actuel une seule fois pour cette itération
            let mut graph_clone = (*self.shared_graph.load_full()).clone();

            for pool_address in hotlist_snapshot {
                if !processed_pools.contains(&pool_address) && !graph_clone.pools.contains_key(&pool_address) {
                    if let Some(unhydrated_pool) = self.main_graph.pools.get(&pool_address) {
                        match Graph::hydrate_pool(unhydrated_pool.clone(), &self.rpc_client).await {
                            Ok(hydrated_pool) => {
                                if let Err(e) = ensure_atas_exist_for_pool(&self.rpc_client, &self.payer, &hydrated_pool).await {
                                    eprintln!("[Producer/Onboard] ERREUR ATA pour {}: {}", pool_address, e);
                                    continue;
                                }
                                graph_clone.add_pool_to_graph(hydrated_pool);
                                println!("[Producer/Onboard] Nouveau pool {} ajouté au hot graph.", pool_address);
                                processed_pools.insert(pool_address);
                                graph_changed = true;
                            }
                            Err(e) => eprintln!("[Producer/Onboard] Échec hydratation pour {}: {}", pool_address, e),
                        }
                    }
                }
            }

            if graph_changed {
                self.shared_graph.store(Arc::new(graph_clone));
                let _ = self.update_notifier.send(()); // Notifier les workers
            }
        }
    }

    // Tâche pour écouter les mises à jour de comptes Geyser
    async fn run_geyser_listener(&self) {
        loop {
            if let Err(e) = self.subscribe_and_process().await {
                eprintln!("[Producer/Geyser] Erreur: {}. Reconnexion dans 5s...", e);
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        }
    }

    async fn subscribe_and_process(&self) -> Result<()> {
        let mut client = GeyserGrpcClient::build_from_shared(self.geyser_grpc_url.clone())?.connect().await?;
        let (mut subscribe_tx, mut stream) = client.subscribe().await?;
        let mut watch_to_pool_map: HashMap<Pubkey, Pubkey> = HashMap::new();
        let mut interval = tokio::time::interval(Duration::from_secs(15));

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    let current_graph = self.shared_graph.load_full();
                    let mut accounts_to_watch = HashSet::new();
                    watch_to_pool_map.clear();
                    for (pool_addr, pool) in &current_graph.pools {
                        match pool {
                            Pool::RaydiumClmm(_) | Pool::OrcaWhirlpool(_) | Pool::MeteoraDlmm(_) | Pool::MeteoraDammV2(_) => {
                                accounts_to_watch.insert(*pool_addr);
                                watch_to_pool_map.insert(*pool_addr, *pool_addr);
                            },
                            _ => {
                                let (v_a, v_b) = pool.get_vaults();
                                accounts_to_watch.insert(v_a);
                                accounts_to_watch.insert(v_b);
                                watch_to_pool_map.insert(v_a, *pool_addr);
                                watch_to_pool_map.insert(v_b, *pool_addr);
                            }
                        }
                    }

                    if !accounts_to_watch.is_empty() {
                        let mut accounts_filter = HashMap::new();
                        accounts_filter.insert("accounts".to_string(), SubscribeRequestFilterAccounts {
                            account: accounts_to_watch.into_iter().map(|p| p.to_string()).collect(), owner: vec![], filters: vec![], nonempty_txn_signature: None,
                        });
                        let request = SubscribeRequest { accounts: accounts_filter, commitment: Some(CommitmentLevel::Processed as i32), ..Default::default() };
                        if subscribe_tx.send(request).await.is_err() { bail!("Canal Geyser fermé."); }
                    }
                }
                message_result = stream.next() => {

                    metrics::GEYSER_MESSAGES_RECEIVED.inc();
                    metrics::GEYSER_LAST_MESSAGE_TIMESTAMP.set(Utc::now().timestamp());

                    let message = match message_result { Some(res) => res?, None => break };
                    if let Some(UpdateOneof::Account(account_update)) = message.update_oneof {
                        let rpc_account = account_update.account.context("Message Geyser sans données de compte")?;
                        let account_key = Pubkey::try_from(rpc_account.pubkey).map_err(|e| anyhow!("Clé invalide: {:?}", e))?;

                        if let Some(pool_addr) = watch_to_pool_map.get(&account_key) {
                            // C'est ici que la magie opère : cloner, modifier, et stocker
                            let old_graph = self.shared_graph.load_full();
                            let mut new_graph = (*old_graph).clone();
                            if let Some(pool_to_update) = new_graph.pools.get_mut(pool_addr) {
                                if pool_to_update.update_from_account_data(&account_key, &rpc_account.data).is_ok() {
                                    self.shared_graph.store(Arc::new(new_graph));
                                    let _ = self.update_notifier.send(()); // Notifier les workers
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


async fn scout_worker(
    shared_graph: Arc<ArcSwap<Graph>>,
    mut update_receiver: watch::Receiver<()>,
    opportunity_sender: mpsc::Sender<ArbitrageOpportunity>,
) {
    info!("[Scout] Démarrage du worker de recherche d'opportunités.");
    loop {
        // Attendre une notification de mise à jour du graphe
        if update_receiver.changed().await.is_err() {
            error!("[Scout] Le canal de notification est fermé. Arrêt.");
            break;
        }

        let graph_snapshot = shared_graph.load_full();
        if graph_snapshot.pools.is_empty() {
            continue;
        }

        // Le scout fait le travail de recherche UNE SEULE FOIS
        let opportunities = find_spatial_arbitrage(graph_snapshot.clone()).await;

        if !opportunities.is_empty() {
            info!("[Scout] Trouvé {} opportunités. Envoi aux workers d'analyse...", opportunities.len());
            for opp in opportunities {
                // Envoie chaque opportunité dans le canal
                if let Err(e) = opportunity_sender.send(opp).await {
                    error!("[Scout] Erreur d'envoi au canal des workers, ils se sont probablement arrêtés: {}", e);
                    break;
                }
            }
        }
    }
}


async fn analysis_worker(
    id: usize,
    // Note: Il reçoit maintenant la partie "réception" du canal mpsc
    opportunity_receiver: Arc<Mutex<mpsc::Receiver<ArbitrageOpportunity>>>,
    // Les autres dépendances restent les mêmes
    shared_graph: Arc<ArcSwap<Graph>>,
    rpc_client: Arc<ResilientRpcClient>,
    payer: Keypair,
    slot_tracker: Arc<SlotTracker>,
    slot_metronome: Arc<SlotMetronome>,
    leader_schedule_tracker: Arc<LeaderScheduleTracker>,
    validator_intel: Arc<ValidatorIntelService>,
    fee_manager: FeeManager,
) {
    info!("[Worker {}] Démarrage.", id);
    // La boucle principale est maintenant beaucoup plus simple
    loop {
        // Attend de recevoir une opportunité du canal.
        // Le Mutex garantit qu'un seul worker à la fois essaie de prendre un message.
        let opportunity = match opportunity_receiver.lock().await.recv().await {
            Some(opp) => opp,
            None => {
                info!("[Worker {}] Le canal d'opportunités est fermé. Arrêt.", id);
                break; // Le canal est fermé, on termine la boucle
            }
        };

        // Une fois qu'on a une opportunité, on la traite.
        // On n'a plus besoin du `currently_processing` Mutex.

        let graph_snapshot = shared_graph.load_full();

        // Réhydratation des pools CLMM si nécessaire (cette logique reste)
        let pool_buy_from_data = match graph_snapshot.pools.get(&opportunity.pool_buy_from_key) {
            Some(p) => p.clone(),
            None => continue,
        };
        let pool_sell_to_data = match graph_snapshot.pools.get(&opportunity.pool_sell_to_key) {
            Some(p) => p.clone(),
            None => continue,
        };

        let rehydrated_buy = rehydrate_if_clmm(pool_buy_from_data, &rpc_client).await;
        let rehydrated_sell = rehydrate_if_clmm(pool_sell_to_data, &rpc_client).await;

        if let (Ok(Some(new_buy)), Ok(Some(new_sell))) = (rehydrated_buy, rehydrated_sell) {
            let mut temp_graph_for_processing = Graph::new();
            temp_graph_for_processing.add_pool_to_graph(new_buy);
            temp_graph_for_processing.add_pool_to_graph(new_sell);

            process_opportunity(
                opportunity, Arc::new(temp_graph_for_processing), rpc_client.clone(), payer.insecure_clone(), &fee_manager,
                slot_tracker.clone(), slot_metronome.clone(), leader_schedule_tracker.clone(), validator_intel.clone()
            ).await;
        } else {
            process_opportunity(
                opportunity, graph_snapshot.clone(), rpc_client.clone(), payer.insecure_clone(), &fee_manager,
                slot_tracker.clone(), slot_metronome.clone(), leader_schedule_tracker.clone(), validator_intel.clone()
            ).await;
        }
    }
}

// Helper pour réhydrater uniquement les pools qui en ont besoin (CLMMs)
async fn rehydrate_if_clmm(pool: Pool, rpc_client: &Arc<ResilientRpcClient>) -> Result<Option<Pool>> {
    match pool {
        Pool::RaydiumClmm(mut p) => {
            mev::decoders::raydium::clmm::hydrate(&mut p, rpc_client).await?;
            Ok(Some(Pool::RaydiumClmm(p)))
        }
        Pool::OrcaWhirlpool(mut p) => {
            mev::decoders::orca::whirlpool::hydrate(&mut p, rpc_client).await?;
            Ok(Some(Pool::OrcaWhirlpool(p)))
        }
        Pool::MeteoraDlmm(mut p) => {
            mev::decoders::meteora::dlmm::hydrate(&mut p, rpc_client).await?;
            Ok(Some(Pool::MeteoraDlmm(p)))
        }
        _ => Ok(None), // Les autres pools n'ont pas besoin de ré-hydratation
    }
}


#[instrument(
    name = "process_opportunity",
    skip_all,
    fields(
        opportunity_id = format!("{}-{}", opportunity.pool_buy_from_key, opportunity.pool_sell_to_key),
        profit_estimation_lamports = opportunity.profit_in_lamports,
        outcome = "pending",
        decision = "N/A"
    )
)]
async fn process_opportunity(
    opportunity: ArbitrageOpportunity,
    graph: Arc<Graph>,
    rpc_client: Arc<ResilientRpcClient>,
    payer: Keypair,
    fee_manager: &FeeManager,
    slot_tracker: Arc<SlotTracker>,
    slot_metronome: Arc<SlotMetronome>,
    leader_schedule_tracker: Arc<LeaderScheduleTracker>,
    validator_intel: Arc<ValidatorIntelService>,
) {
    let start_time = Instant::now();
    let current_span = tracing::Span::current();

    // --- NOUVEAU : Créer un identifiant canonique pour la paire de pools ---
    let mut pools = [opportunity.pool_buy_from_key.to_string(), opportunity.pool_sell_to_key.to_string()];
    pools.sort();
    let pool_pair_id = format!("{}-{}", pools[0], pools[1]);
    // --- FIN DU NOUVEAU ---

    // --- PHASE 1: Estimation Locale ---
    info!("Début du traitement de l'opportunité.");
    let pool_buy_from = match graph.pools.get(&opportunity.pool_buy_from_key) {
        Some(p) => p.clone(),
        None => {
            error!("Pool (buy) non trouvé dans le graphe.");
            current_span.record("outcome", "InternalError");
            metrics::TRANSACTION_OUTCOMES.with_label_values(&["InternalError", &pool_pair_id]).inc();
            metrics::PROCESS_OPPORTUNITY_LATENCY.observe(start_time.elapsed().as_secs_f64());
            return;
        }
    };

    let pool_sell_to = match graph.pools.get(&opportunity.pool_sell_to_key) {
        Some(p) => p.clone(),
        None => {
            error!("Pool (sell) non trouvé dans le graphe.");
            current_span.record("outcome", "InternalError");
            metrics::TRANSACTION_OUTCOMES.with_label_values(&["InternalError", &pool_pair_id]).inc();
            metrics::PROCESS_OPPORTUNITY_LATENCY.observe(start_time.elapsed().as_secs_f64());
            return;
        }
    };
    let current_timestamp = slot_tracker.current().clock.unix_timestamp;
    let quote_buy_details = match pool_buy_from.get_quote_with_details(&opportunity.token_in_mint, opportunity.amount_in, current_timestamp) {
        Ok(q) => q,
        Err(e) => {
            warn!(error = %e, "Échec du calcul de quote (buy). Abandon.");
            current_span.record("outcome", "QuoteError");
            metrics::TRANSACTION_OUTCOMES.with_label_values(&["QuoteError", &pool_pair_id]).inc();
            metrics::PROCESS_OPPORTUNITY_LATENCY.observe(start_time.elapsed().as_secs_f64());
            return;
        }
    };
    let quote_sell_details = match pool_sell_to.get_quote_with_details(&opportunity.token_intermediate_mint, quote_buy_details.amount_out, current_timestamp) {
        Ok(q) => q,
        Err(e) => {
            warn!(error = %e, "Échec du calcul de quote (sell). Abandon.");
            current_span.record("outcome", "QuoteError");
            metrics::TRANSACTION_OUTCOMES.with_label_values(&["QuoteError", &pool_pair_id]).inc();
            metrics::PROCESS_OPPORTUNITY_LATENCY.observe(start_time.elapsed().as_secs_f64());
            return;
        }
    };
    let profit_brut_estime = quote_sell_details.amount_out.saturating_sub(opportunity.amount_in);
    if profit_brut_estime < 50000 {
        info!(profit = profit_brut_estime, "Abandon. Profit brut estimé insuffisant.");
        current_span.record("outcome", "Abandoned_ProfitTooLow");
        metrics::TRANSACTION_OUTCOMES.with_label_values(&["Abandoned_ProfitTooLow", &pool_pair_id]).inc();
        metrics::PROCESS_OPPORTUNITY_LATENCY.observe(start_time.elapsed().as_secs_f64());
        return;
    }
    let estimated_cus = cu_manager::estimate_arbitrage_cost(&pool_buy_from, quote_buy_details.ticks_crossed, &pool_sell_to, quote_sell_details.ticks_crossed);
    info!(profit_brut_estime, estimated_cus, "Estimation locale terminée.");
    let protections = match protections::calculate_slippage_protections(opportunity.amount_in, profit_brut_estime, pool_sell_to.clone(), &opportunity.token_intermediate_mint, current_timestamp) {
        Ok(p) => p,
        Err(e) => {
            warn!(error = %e, "Échec du calcul des protections. Abandon.");
            current_span.record("outcome", "ProtectionError");
            metrics::TRANSACTION_OUTCOMES.with_label_values(&["ProtectionError", &pool_pair_id]).inc();
            metrics::PROCESS_OPPORTUNITY_LATENCY.observe(start_time.elapsed().as_secs_f64());
            return;
        }
    };
    let current_slot = slot_tracker.current().clock.slot;
    let time_remaining_in_slot = slot_metronome.estimated_time_remaining_in_slot_ms();
    let (target_slot, leader_identity_opt) = if time_remaining_in_slot > BOT_PROCESSING_TIME_MS {(current_slot, leader_schedule_tracker.get_leader_for_slot(current_slot))} else {let next_slot = current_slot + 1; (next_slot, leader_schedule_tracker.get_leader_for_slot(next_slot))};
    let leader_identity = match leader_identity_opt {
        Some(identity) => identity,
        None => {
            error!(slot = target_slot, "Leader introuvable pour le slot cible. Abandon.");
            current_span.record("outcome", "LeaderNotFound");
            metrics::TRANSACTION_OUTCOMES.with_label_values(&["LeaderNotFound", &pool_pair_id]).inc();
            metrics::PROCESS_OPPORTUNITY_LATENCY.observe(start_time.elapsed().as_secs_f64());
            return;
        }
    };
    let managed_lut_address = match Pubkey::from_str(MANAGED_LUT_ADDRESS) {
        Ok(addr) => addr,
        Err(_) => {
            error!("Adresse de LUT invalide dans le code source.");
            metrics::PROCESS_OPPORTUNITY_LATENCY.observe(start_time.elapsed().as_secs_f64());
            return;
        }
    };
    let lut_account_data = match rpc_client.get_account_data(&managed_lut_address).await {
        Ok(data) => data,
        Err(e) => {
            error!(error = %e, "Le compte de la LUT n'a pas été trouvé.");
            metrics::PROCESS_OPPORTUNITY_LATENCY.observe(start_time.elapsed().as_secs_f64());
            return;
        }
    };
    let lut_ref = match AddressLookupTable::deserialize(&lut_account_data) {
        Ok(table) => table,
        Err(e) => {
            error!(error = %e, "Échec de la désérialisation de la LUT.");
            metrics::PROCESS_OPPORTUNITY_LATENCY.observe(start_time.elapsed().as_secs_f64());
            return;
        }
    };
    let owned_lookup_table = SdkAddressLookupTableAccount { key: managed_lut_address, addresses: lut_ref.addresses.to_vec() };
    let mut final_tx: Option<VersionedTransaction> = None;
    let mut is_jito_leader = false;
    if let Some(_validator_info) = validator_intel.get_validator_info(&leader_identity).await {
        is_jito_leader = true;
        current_span.record("decision", "PrepareJito");
        let jito_tip = fee_manager.calculate_jito_tip(profit_brut_estime, JITO_TIP_PERCENT, estimated_cus);
        let profit_net_final = profit_brut_estime.saturating_sub(jito_tip);
        if profit_net_final > 5000 {
            info!(profit_net_final, jito_tip, "DÉCISION : PRÉPARER BUNDLE JITO.");
            match transaction_builder::build_arbitrage_transaction(&opportunity, graph, &rpc_client, &payer, &owned_lookup_table, &protections, estimated_cus, 0).await {
                Ok((tx, _)) => final_tx = Some(tx),
                Err(e) => {
                    error!(error = %e, "Échec de la construction de la transaction Jito.");
                    current_span.record("outcome", "BuildError");
                    metrics::TRANSACTION_OUTCOMES.with_label_values(&["BuildError", &pool_pair_id]).inc();
                }
            }
        } else {
            info!(profit_net_final, "DÉCISION : Abandon. Profit Jito insuffisant.");
            current_span.record("outcome", "Abandoned_JitoProfitTooLow");
            metrics::TRANSACTION_OUTCOMES.with_label_values(&["Abandoned_JitoProfitTooLow", &pool_pair_id]).inc();
        }
    } else {
        current_span.record("decision", "PrepareNormal");
        const OVERBID_PERCENT: u8 = 20;
        let accounts_in_tx = vec![opportunity.pool_buy_from_key, opportunity.pool_sell_to_key];
        let priority_fee_price_per_cu = fee_manager.calculate_priority_fee(&accounts_in_tx, OVERBID_PERCENT).await;
        let total_priority_fee = (estimated_cus * priority_fee_price_per_cu) / 1_000_000;
        let profit_net_final = profit_brut_estime.saturating_sub(5000).saturating_sub(total_priority_fee);
        if profit_net_final > 5000 {
            info!(profit_net_final, total_priority_fee, "DÉCISION: PRÉPARER TRANSACTION NORMALE.");
            match transaction_builder::build_arbitrage_transaction(&opportunity, graph, &rpc_client, &payer, &owned_lookup_table, &protections, estimated_cus, priority_fee_price_per_cu).await {
                Ok((tx, _)) => final_tx = Some(tx),
                Err(e) => {
                    error!(error = %e, "Échec de la construction de la transaction normale.");
                    current_span.record("outcome", "BuildError");
                    metrics::TRANSACTION_OUTCOMES.with_label_values(&["BuildError", &pool_pair_id]).inc();
                }
            }
        } else {
            info!(profit_net_final, "DÉCISION : Abandon. Profit normal insuffisant.");
            current_span.record("outcome", "Abandoned_NormalProfitTooLow");
            metrics::TRANSACTION_OUTCOMES.with_label_values(&["Abandoned_NormalProfitTooLow", &pool_pair_id]).inc();
        }
    }
    if let Some(tx_to_simulate) = final_tx {
        let sim_config = RpcSimulateTransactionConfig {sig_verify: false, replace_recent_blockhash: true, commitment: Some(rpc_client.commitment()), encoding: Some(UiTransactionEncoding::Base64), accounts: None, min_context_slot: None, inner_instructions: false,};
        match rpc_client.simulate_transaction_with_config(&tx_to_simulate, sim_config).await {
            Ok(sim_response) => {
                let sim_value = sim_response.value;
                if let Some(err) = sim_value.err {
                    warn!(error = ?err, "ÉCHEC DE LA SIMULATION FINALE.");
                    if let Some(logs) = sim_value.logs { for log in logs { warn!(log = log); } }
                    current_span.record("outcome", "SimFailure");
                    metrics::TRANSACTION_OUTCOMES.with_label_values(&["SimFailure", &pool_pair_id]).inc();
                } else {
                    info!("SUCCÈS DE LA SIMULATION FINALE !");
                    current_span.record("outcome", "SimSuccess");
                    metrics::TRANSACTION_OUTCOMES.with_label_values(&["SimSuccess", &pool_pair_id]).inc();
                    if let Some(logs) = &sim_value.logs {
                        if let Some(realized_profit) = simulator::parse_realized_profit_from_logs(logs) {
                            let slippage = (profit_brut_estime as i64) - (realized_profit as i64);
                            info!(realized_profit, slippage, "Analyse financière (basée sur la simulation) terminée.");
                            metrics::ARBITRAGE_PROFIT_REALIZED.observe(realized_profit as f64);
                            metrics::ARBITRAGE_SLIPPAGE.observe(slippage as f64);
                        }
                    }
                    if is_jito_leader {
                        info!("[PLACEHOLDER] La transaction Jito serait envoyée ici.");
                        metrics::TRANSACTIONS_SENT.inc();
                        current_span.record("decision", "JitoSent_Simulated");
                        metrics::TRANSACTION_OUTCOMES.with_label_values(&["JitoSent_Simulated", &pool_pair_id]).inc();
                    } else {
                        info!("[PLACEHOLDER] La transaction normale serait envoyée ici.");
                        metrics::TRANSACTIONS_SENT.inc();
                        current_span.record("decision", "NormalSent_Simulated");
                        metrics::TRANSACTION_OUTCOMES.with_label_values(&["NormalSent_Simulated", &pool_pair_id]).inc();
                    }
                }
            },
            Err(e) => {
                error!(error = %e, "ERREUR RPC pendant la simulation finale.");
                current_span.record("outcome", "RpcError");
                metrics::TRANSACTION_OUTCOMES.with_label_values(&["RpcError", &pool_pair_id]).inc();
            }
        }
    }
    metrics::PROCESS_OPPORTUNITY_LATENCY.observe(start_time.elapsed().as_secs_f64());
}


#[tokio::main]
async fn main() -> Result<()> {
    println!("--- Lancement de l'Execution Bot (Architecture Scout-Worker) ---");
    dotenvy::dotenv().ok();

    // --- Initialisation (inchangée) ---
    let config = Config::load()?;
    let payer = Keypair::from_base58_string(&config.payer_private_key);
    let rpc_client = Arc::new(ResilientRpcClient::new(config.solana_rpc_url.clone(), 3, 500));
    println!("[Init] Le bot utilisera la LUT gérée à l'adresse: {}", MANAGED_LUT_ADDRESS);
    let geyser_url = env::var("GEYSER_GRPC_URL").context("GEYSER_GRPC_URL requis")?;
    let validators_app_token = env::var("VALIDATORS_APP_API_KEY").context("VALIDATORS_APP_API_KEY requis")?;
    println!("[Init] Le bot utilisera la LUT gérée à l'adresse: {}", MANAGED_LUT_ADDRESS);

    println!("[Init] Services de suivi de la blockchain...");
    let validator_intel_service = Arc::new(ValidatorIntelService::new(validators_app_token.clone()).await?);
    validator_intel_service.start(validators_app_token);
    let slot_tracker = Arc::new(SlotTracker::new(&rpc_client).await?);
    slot_tracker.start(geyser_url.clone(), rpc_client.clone());
    let leader_schedule_tracker = Arc::new(LeaderScheduleTracker::new(rpc_client.clone()).await?);
    leader_schedule_tracker.start();
    let slot_metronome = Arc::new(SlotMetronome::new(slot_tracker.clone()));
    slot_metronome.start();

    ensure_pump_user_account_exists(&rpc_client, &payer).await?;
    let main_graph = Arc::new(load_main_graph_from_cache()?);

    // --- NOUVELLE ARCHITECTURE ---
    let hot_graph = Arc::new(ArcSwap::new(Arc::new(Graph::new())));
    let (update_tx, update_rx) = watch::channel(());
    let shared_hotlist = Arc::new(tokio::sync::RwLock::new(HashSet::<Pubkey>::new()));

    // --- NOUVEAU : Création du canal (UNE SEULE FOIS) ---
    let (opportunity_tx, opportunity_rx) = mpsc::channel::<ArbitrageOpportunity>(1024);
    let shared_opportunity_rx = Arc::new(Mutex::new(opportunity_rx));

    // Tâche 1 : Mettre à jour la `shared_hotlist` depuis le fichier (inchangé)
    let hotlist_updater_clone = shared_hotlist.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(2));
        loop { interval.tick().await; if let Ok(list) = read_hotlist() { *hotlist_updater_clone.write().await = list; } }
    });

    // Tâche 2 : Le FeeManager (inchangé)
    let fee_manager = FeeManager::new(rpc_client.clone());
    fee_manager.start(shared_hotlist.clone());

    // Tâche 3 : Le Producteur de données
    let producer = DataProducer {
        geyser_grpc_url: geyser_url.clone(),
        shared_graph: hot_graph.clone(),
        main_graph: main_graph.clone(),
        shared_hotlist: shared_hotlist.clone(),
        update_notifier: update_tx,
        rpc_client: rpc_client.clone(),
        payer: Keypair::try_from(payer.to_bytes().as_slice())?,
    };
    tokio::spawn(async move {
        producer.run().await;
    });

    // --- NOUVEAU : Tâche 4 - Le Scout ---
    let scout_graph = hot_graph.clone();
    let scout_rx = update_rx.clone();
    tokio::spawn(async move {
        // Cette variable `opportunity_tx` est maintenant celle du premier (et unique) canal
        scout_worker(scout_graph, scout_rx, opportunity_tx).await;
    });

    // --- MODIFIÉ : Tâche 5 - Les Workers d'analyse ---
    println!("[Init] Démarrage de {} workers d'analyse...", ANALYSIS_WORKER_COUNT);
    for i in 0..ANALYSIS_WORKER_COUNT {
        let worker_rx = shared_opportunity_rx.clone(); // Clone l'Arc, pas le receiver
        let worker_graph = hot_graph.clone();
        let worker_rpc = rpc_client.clone();
        let worker_payer = Keypair::try_from(payer.to_bytes().as_slice())?;
        let worker_slot_tracker = slot_tracker.clone();
        let worker_slot_metronome = slot_metronome.clone();
        let worker_leader_schedule = leader_schedule_tracker.clone();
        let worker_validator_intel = validator_intel_service.clone();
        let worker_fee_manager = fee_manager.clone();

        tokio::spawn(async move {
            analysis_worker(
                i + 1, worker_rx, worker_graph, worker_rpc, worker_payer,
                worker_slot_tracker, worker_slot_metronome,
                worker_leader_schedule, worker_validator_intel, worker_fee_manager,
            ).await;
        });
    }

    println!("[Bot] Architecture démarrée. Le bot est opérationnel.");
    std::future::pending::<()>().await;

    Ok(())
}