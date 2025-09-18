#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

// --- CORRECTION : Imports nettoyés et corrigés ---
use anyhow::{anyhow, bail, Context, Result};
use arc_swap::ArcSwap;
use chrono::Utc;
use futures_util::{sink::SinkExt, StreamExt};
use spl_associated_token_account::instruction::create_associated_token_account;
use solana_sdk::{
    pubkey::Pubkey, signature::Keypair, signer::Signer, transaction::Transaction,
    transaction::VersionedTransaction,
};
use mev::execution::confirmation_service::ConfirmationService;
use mev::state::balance_manager;
use tracing::warn;
use std::{
    collections::{HashMap, HashSet},
    env, fs,
    io::Read,
    sync::{Arc, RwLock},
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, Mutex, watch};
use tracing::{error, info};
use yellowstone_grpc_client::GeyserGrpcClient;
use yellowstone_grpc_proto::prelude::{
    subscribe_update::UpdateOneof, CommitmentLevel, SubscribeRequest,
    SubscribeRequestFilterAccounts,
};

use mev::{
    config::Config,
    decoders::{Pool, PoolOperations, meteora, orca, raydium},
    execution::{
        cu_manager,
        fee_manager::FeeManager,
        sender::{TransactionSender, UnifiedSender},
        transaction_builder::ArbitrageInstructionsTemplate, // Correct path
    },
    graph_engine::Graph,
    middleware::{
        final_simulator::FinalSimulator, protection_calculator::ProtectionCalculator,
        quote_validator::QuoteValidator, transaction_builder::TransactionBuilder,
        ExecutionContext, Pipeline,
    },
    monitoring::metrics,
    rpc::ResilientRpcClient,
    state::{
        balance_tracker,
        leader_schedule::LeaderScheduleTracker,
        slot_metronome::SlotMetronome,
        slot_tracker::SlotTracker,
        validator_intel::ValidatorIntelService,
    },
    strategies::spatial::{find_spatial_arbitrage, ArbitrageOpportunity},
};

const MANAGED_LUT_ADDRESS: &str = "E5h798UBdK8V1L7MvRfi1ppr2vitPUUUUCVqvTyDgKXN";
const ANALYSIS_WORKER_COUNT: usize = 4;


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


const FAILURE_THRESHOLD: usize = 5; // Nombre d'échecs consécutifs avant de déclencher
const COOLDOWN_SECONDS: u64 = 3600; // MODIFIÉ : 1 heure de pause
const BLACKLIST_THRESHOLD: usize = 3; // NOUVEAU : Nombre de pauses avant de blacklister
const BLACKLIST_FILE_NAME: &str = "pool_pair_blacklist.json"; // NOUVEAU : Fichier de persistance

// Structure pour suivre les échecs et l'état de pause
struct FailureTracker {
    consecutive_failures: usize,
    cooldown_until: Option<Instant>,
    cooldown_trigger_count: usize, // NOUVEAU : Compte le nombre de fois où la pause a été activée
}

// NOUVELLE STRUCTURE
#[derive(Clone)]
struct BlacklistManager {
    blacklisted_pairs: Arc<RwLock<HashSet<String>>>,
}

impl BlacklistManager {
    // Charge la blacklist depuis le fichier au démarrage
    fn load() -> Self {
        let blacklisted_pairs = match fs::read_to_string(BLACKLIST_FILE_NAME) {
            Ok(data) => serde_json::from_str(&data).unwrap_or_else(|e| {
                warn!("Impossible de parser le fichier blacklist.json (erreur: {}), démarrage avec une liste vide.", e);
                HashSet::new()
            }),
            Err(_) => {
                info!("Fichier blacklist.json non trouvé, démarrage avec une liste vide.");
                HashSet::new()
            }
        };
        info!("{} paires chargées depuis la blacklist.", blacklisted_pairs.len());
        Self {
            blacklisted_pairs: Arc::new(RwLock::new(blacklisted_pairs)),
        }
    }

    // Ajoute une paire à la blacklist et sauvegarde sur le disque
    fn add(&self, pool_pair_id: &str) {
        let mut writer = self.blacklisted_pairs.write().unwrap();
        if writer.insert(pool_pair_id.to_string()) {
            // Sauvegarde uniquement si un nouvel élément a été ajouté
            if let Err(e) = fs::write(
                BLACKLIST_FILE_NAME,
                serde_json::to_string_pretty(&*writer).unwrap(),
            ) {
                error!("Échec de la sauvegarde de la blacklist dans le fichier : {}", e);
            }
        }
    }

    // Vérifie si une paire est blacklistée
    fn is_blacklisted(&self, pool_pair_id: &str) -> bool {
        self.blacklisted_pairs.read().unwrap().contains(pool_pair_id)
    }
}


struct DataProducer {
    geyser_grpc_url: String,
    shared_graph: Arc<ArcSwap<Graph>>,
    main_graph: Arc<Graph>,
    shared_hotlist: Arc<tokio::sync::RwLock<HashSet<Pubkey>>>,
    update_notifier: watch::Sender<()>,
    rpc_client: Arc<ResilientRpcClient>,
    payer: Keypair,
}

impl DataProducer {
    async fn run(&self) {
        println!("[Producer] Démarrage du service de mise à jour du graphe.");
        let geyser_task = self.run_geyser_listener();
        let onboarding_task = self.run_onboarding_manager();
        let pre_hydration_task = self.run_clmm_pre_hydrator();
        tokio::join!(geyser_task, onboarding_task, pre_hydration_task);
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
                            Ok(mut hydrated_pool) => {
                                if let Err(e) = ensure_atas_exist_for_pool(&self.rpc_client, &self.payer, &hydrated_pool).await {
                                    eprintln!("[Producer/Onboard] ERREUR ATA pour {}: {}", pool_address, e);
                                    continue;
                                }

                                // --- MISE À JOUR DU BLOC DE PEUPLEMENT ---
                                match &mut hydrated_pool {
                                    Pool::RaydiumClmm(p) => {
                                        p.populate_quote_lookup_table(true);
                                        p.populate_quote_lookup_table(false);
                                    }
                                    Pool::OrcaWhirlpool(p) => {
                                        p.populate_quote_lookup_table(true);
                                        p.populate_quote_lookup_table(false);
                                    }
                                    Pool::MeteoraDlmm(p) => {
                                        p.populate_quote_lookup_table(true);
                                        p.populate_quote_lookup_table(false);
                                    }
                                    Pool::MeteoraDammV2(p) => {
                                        p.populate_quote_lookup_table(true);
                                        p.populate_quote_lookup_table(false);
                                    }
                                    _ => {} // Ne rien faire pour les autres types
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

    async fn run_clmm_pre_hydrator(&self) {
        let mut interval = tokio::time::interval(Duration::from_secs(2));
        loop {
            interval.tick().await;

            let old_graph = self.shared_graph.load_full();
            if old_graph.pools.is_empty() {
                continue;
            }
            let mut new_graph = (*old_graph).clone();
            let mut graph_was_updated = false;
            let mut hydration_futures = Vec::new();

            for (pool_key, pool) in new_graph.pools.iter() {
                let is_clmm = matches!(pool, Pool::RaydiumClmm(_) | Pool::OrcaWhirlpool(_) | Pool::MeteoraDlmm(_) | Pool::MeteoraDammV2(_));
                if is_clmm {
                    let pool_clone = pool.clone();
                    let rpc_client_clone = self.rpc_client.clone();
                    let key_clone = *pool_key;

                    hydration_futures.push(tokio::spawn(async move {
                        // On réutilise la même logique d'hydratation que dans les décodeurs
                        let hydrated_pool_result: Result<Pool, anyhow::Error> = async {
                            match pool_clone {
                                Pool::RaydiumClmm(mut p) => {
                                    raydium::clmm::hydrate(&mut p, &rpc_client_clone).await?;
                                    Ok(Pool::RaydiumClmm(p))
                                },
                                Pool::OrcaWhirlpool(mut p) => {
                                    orca::whirlpool::hydrate(&mut p, &rpc_client_clone).await?;
                                    Ok(Pool::OrcaWhirlpool(p))
                                },
                                Pool::MeteoraDlmm(mut p) => {
                                    meteora::dlmm::hydrate(&mut p, &rpc_client_clone).await?;
                                    Ok(Pool::MeteoraDlmm(p))
                                },
                                Pool::MeteoraDammV2(mut p) => {
                                    meteora::damm_v2::hydrate(&mut p, &rpc_client_clone).await?;
                                    Ok(Pool::MeteoraDammV2(p))
                                }
                                _ => unreachable!(),
                            }
                        }.await;
                        (key_clone, hydrated_pool_result)
                    }));
                }
            }

            let results = futures_util::future::join_all(hydration_futures).await;

            for result in results {
                if let Ok((key, Ok(hydrated_pool))) = result {
                    new_graph.update_pool_in_graph(&key, hydrated_pool);
                    graph_was_updated = true;
                }
            }

            if graph_was_updated {
                self.shared_graph.store(Arc::new(new_graph));
                let _ = self.update_notifier.send(());
            }
        }
    }
}


async fn scout_worker(
    shared_graph: Arc<ArcSwap<Graph>>,
    mut update_receiver: watch::Receiver<()>,
    opportunity_sender: mpsc::Sender<ArbitrageOpportunity>,
    config: Config,
) {
    info!("[Scout] Démarrage du worker de recherche d'opportunités.");
    loop {
        if update_receiver.changed().await.is_err() {
            error!("[Scout] Le canal de notification est fermé. Arrêt.");
            break;
        }

        let graph_snapshot = shared_graph.load_full();
        if graph_snapshot.pools.is_empty() {
            continue;
        }

        let mut opportunities = find_spatial_arbitrage(graph_snapshot.clone(), &config).await;

        if !opportunities.is_empty() {
            // --- NOUVEAU BLOC DE SCORING ET DE TRI ---
            info!("[Scout] {} opportunités brutes trouvées. Calcul des scores et tri...", opportunities.len());

            opportunities.sort_by_cached_key(|opp| {
                // On estime un coût de transaction très simple ici.
                // NOTE : C'est une estimation rapide, pas la valeur finale.
                // Le but est juste de classer les opportunités.

                // On récupère les pools pour l'estimation de CUs.
                let pool_buy_from = graph_snapshot.pools.get(&opp.pool_buy_from_key);
                let pool_sell_to = graph_snapshot.pools.get(&opp.pool_sell_to_key);

                let estimated_cost = if let (Some(buy_pool), Some(sell_pool)) = (pool_buy_from, pool_sell_to) {
                    // On suppose 1 tick traversé pour une estimation rapide.
                    // Le vrai nombre sera calculé par le `QuoteValidator`.
                    let estimated_cus = cu_manager::estimate_arbitrage_cost(buy_pool, 1, sell_pool, 1);

                    // Estimation agressive du coût des frais pour le scoring.
                    const ESTIMATED_PRIORITY_FEE_PER_MILLION_CU: u64 = 5000;
                    (estimated_cus * ESTIMATED_PRIORITY_FEE_PER_MILLION_CU) / 1_000_000
                } else {
                    // Si on ne trouve pas les pools, on pénalise lourdement.
                    u64::MAX
                };

                let score = (opp.profit_in_lamports as i128) - (estimated_cost as i128);

                // On veut trier du plus grand score au plus petit, donc on inverse le score.
                -score
            });

            info!("[Scout] Tri terminé. Meilleur score: ~{} lamports de profit net estimé.",
                  -(opportunities.first().map_or(0, |opp| ((opp.profit_in_lamports as i128) - 10000))));

            // --- FIN DU NOUVEAU BLOC ---

            for opp in opportunities {
                if let Err(e) = opportunity_sender.send(opp).await {
                    error!("[Scout] Erreur d'envoi au canal des workers, ils se sont probablement arrêtés: {}", e);
                    break;
                }
            }
        }
    }
}



#[derive(Clone)] // On dit à Rust comment cloner cette structure
struct WorkerDependencies {
    rpc_client: Arc<ResilientRpcClient>,
    slot_tracker: Arc<SlotTracker>,
    slot_metronome: Arc<SlotMetronome>,
    leader_schedule_tracker: Arc<LeaderScheduleTracker>,
    validator_intel: Arc<ValidatorIntelService>,
    fee_manager: FeeManager,
    sender: Arc<dyn TransactionSender>,
    config: Config,
    template_cache: Arc<RwLock<HashMap<String, ArbitrageInstructionsTemplate>>>,
}

async fn analysis_worker(
    id: usize,
    opportunity_receiver: Arc<Mutex<mpsc::Receiver<ArbitrageOpportunity>>>,
    shared_graph: Arc<ArcSwap<Graph>>,
    payer: Keypair,
    deps: WorkerDependencies,
    circuit_breaker_state: Arc<RwLock<HashMap<String, FailureTracker>>>,
    blacklist_manager: BlacklistManager,
) {
    info!("[Worker {}] Démarrage.", id);

    let pipeline = Pipeline::new(vec![
        Box::new(QuoteValidator),
        Box::new(ProtectionCalculator),
        Box::new(TransactionBuilder::new(
            deps.slot_tracker.clone(),
            deps.slot_metronome.clone(),
            deps.leader_schedule_tracker.clone(),
            deps.validator_intel.clone(),
            deps.fee_manager.clone(),
            // --- ON PASSE LE CACHE DE TEMPLATES ICI ---
            deps.template_cache.clone(),
        )),
        Box::new(FinalSimulator::new(deps.sender.clone())),
    ]);

    loop {
        let opportunity = match opportunity_receiver.lock().await.recv().await {
            Some(opp) => opp,
            None => {
                info!("[Worker {}] Le canal d'opportunités est fermé. Arrêt.", id);
                break;
            }
        };

        let start_time = Instant::now();
        let graph_for_processing = shared_graph.load_full();

        let pool_pair_id = format!(
            "{}-{}",
            opportunity.pool_buy_from_key.min(opportunity.pool_sell_to_key),
            opportunity.pool_buy_from_key.max(opportunity.pool_sell_to_key)
        );

        // --- NIVEAU 3 : VÉRIFICATION DE LA BLACKLIST (LA PLUS RAPIDE) ---
        if blacklist_manager.is_blacklisted(&pool_pair_id) {
            continue; // Ignore silencieusement, car c'est permanent
        }

        // --- DÉBUT DE LA LOGIQUE DU DISJONCTEUR ---
        let is_in_cooldown = {
            let reader = circuit_breaker_state.read().unwrap();
            if let Some(tracker) = reader.get(&pool_pair_id) {
                if let Some(cooldown_until) = tracker.cooldown_until {
                    if Instant::now() < cooldown_until {
                        // Toujours en pause
                        true
                    } else {
                        // La pause est terminée, on peut réessayer
                        false
                    }
                } else {
                    false
                }
            } else {
                false
            }
        };

        if is_in_cooldown {
            info!("[Worker {}] Paire {} en pause. Opportunité ignorée.", id, pool_pair_id);
            continue; // On passe à la prochaine opportunité
        }
        // --- FIN DE LA LOGIQUE DU DISJONCTEUR ---

        let span = tracing::info_span!(
            "process_opportunity",
            opportunity_id = format!("{}-{}", opportunity.pool_buy_from_key, opportunity.pool_sell_to_key),
            profit_estimation_lamports = opportunity.profit_in_lamports,
            outcome = "pending",
            decision = "N/A"
        );
        let _enter = span.enter();

        let current_timestamp = deps.slot_tracker.current().clock.unix_timestamp;

        let context = ExecutionContext::new(
            opportunity,
            graph_for_processing.clone(), // On clone simplement l'Arc
            payer.insecure_clone(),
            deps.rpc_client.clone(),
            current_timestamp,
            span.clone(),
            deps.config.clone(),
        );

        let pipeline_result = pipeline.run(context).await;

        let mut writer = circuit_breaker_state.write().unwrap();
        let tracker = writer.entry(pool_pair_id.clone()).or_insert(FailureTracker {
            consecutive_failures: 0,
            cooldown_until: None,
            cooldown_trigger_count: 0, // Initialisation
        });

        if tracker.cooldown_until.is_some() && Instant::now() > tracker.cooldown_until.unwrap() {
            info!("[Worker {}] Fin de la pause pour la paire {}.", id, pool_pair_id);
            tracker.consecutive_failures = 0;
            tracker.cooldown_until = None;
        }

        if pipeline_result.is_err() {
            tracker.consecutive_failures += 1;
            info!("[Worker {}] Échec détecté pour la paire {}. Échecs consécutifs: {}", id, pool_pair_id, tracker.consecutive_failures);

            if tracker.consecutive_failures >= FAILURE_THRESHOLD {
                tracker.cooldown_trigger_count += 1; // On incrémente le compteur de déclenchements
                warn!(
                    "[Worker {}] DISJONCTEUR DÉCLENCHÉ ({}ème fois) pour la paire {} ! Mise en pause pour {} secondes.",
                    id, tracker.cooldown_trigger_count, pool_pair_id, COOLDOWN_SECONDS
                );
                tracker.cooldown_until = Some(Instant::now() + Duration::from_secs(COOLDOWN_SECONDS));
                metrics::CIRCUIT_BREAKER_TRIPPED.with_label_values(&[&pool_pair_id]).inc();

                // LOGIQUE D'ESCALADE -> BLACKLIST
                if tracker.cooldown_trigger_count >= BLACKLIST_THRESHOLD {
                    error!(
                        "[Worker {}] SEUIL DE BLACKLIST ATTEINT pour la paire {} ! Ajout permanent à la liste noire.",
                        id, pool_pair_id
                    );
                    blacklist_manager.add(&pool_pair_id);
                }
            }
        } else {
            if tracker.consecutive_failures > 0 {
                info!("[Worker {}] Succès détecté pour la paire {}. Réinitialisation du compteur d'échecs.", id, pool_pair_id);
            }
            tracker.consecutive_failures = 0;
        }
        // --- FIN MISE À JOUR DU TRACKER ---

        metrics::PROCESS_OPPORTUNITY_LATENCY.observe(start_time.elapsed().as_secs_f64());
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    println!("--- Lancement de l'Execution Bot (Architecture Scout-Worker) ---");
    dotenvy::dotenv().ok();

    // --- Initialisation ---
    let config = Config::load()?;
    // On crée une copie de la keypair spécifiquement pour le manager
    let payer_for_manager = Keypair::from_base58_string(&config.payer_private_key);
    let payer = Keypair::from_base58_string(&config.payer_private_key);
    let rpc_client = Arc::new(ResilientRpcClient::new(config.solana_rpc_url.clone(), 3, 500));
    let geyser_url = env::var("GEYSER_GRPC_URL").context("GEYSER_GRPC_URL requis")?;
    let validators_app_token = env::var("VALIDATORS_APP_API_KEY").context("VALIDATORS_APP_API_KEY requis")?;
    balance_tracker::start_monitoring(rpc_client.clone(), config.clone());
    println!("[Init] Le bot utilisera la LUT gérée à l'adresse: {}", MANAGED_LUT_ADDRESS);

    // --- NOUVEAU : Démarrer le gestionnaire de solde en tâche de fond ---
    balance_manager::start_balance_manager(rpc_client.clone(), payer_for_manager, config.clone());
    println!("[Init] Service de gestion du solde SOL/WSOL démarré.");

    balance_tracker::start_monitoring(rpc_client.clone(), config.clone());
    println!("[Init] Le bot utilisera la LUT gérée à l'adresse: {}", MANAGED_LUT_ADDRESS);

    println!("[Init] Démarrage du service de confirmation de P&L...");
    let (confirmation_service, tx_to_confirm_sender) = ConfirmationService::new(rpc_client.clone());
    confirmation_service.start();

    // --- Instanciation du service d'envoi unifié ---
    // Mettez `dry_run: true` pour tester localement sans rien envoyer.
    // Mettez `dry_run: false` sur votre bare metal pour envoyer réellement les transactions.
    let sender: Arc<dyn TransactionSender> = Arc::new(UnifiedSender::new(
        rpc_client.clone(),
        true, // dry_run: mettre à `false` en production
        // --- ON PASSE LE SENDER DU CANAL ICI ---
        tx_to_confirm_sender,
    ));
    println!("[Init] Service d'envoi de transactions configuré (Mode: Dry Run).");


    // --- Services de suivi de la blockchain ---
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

    // --- Architecture ---
    let hot_graph = Arc::new(ArcSwap::new(Arc::new(Graph::new())));
    let (update_tx, update_rx) = watch::channel(());
    let shared_hotlist = Arc::new(tokio::sync::RwLock::new(HashSet::<Pubkey>::new()));
    let (opportunity_tx, opportunity_rx) = mpsc::channel::<ArbitrageOpportunity>(1024);
    let shared_opportunity_rx = Arc::new(Mutex::new(opportunity_rx));

    // Tâche 1 : Hotlist Updater
    let hotlist_updater_clone = shared_hotlist.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(2));
        loop { interval.tick().await; if let Ok(list) = read_hotlist() { *hotlist_updater_clone.write().await = list; } }
    });

    // Tâche 2 : FeeManager
    let fee_manager = FeeManager::new(rpc_client.clone());
    fee_manager.start(shared_hotlist.clone());

    // Tâche 3 : Data Producer
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

    // Tâche 4 : Scout
    let scout_graph = hot_graph.clone();
    let scout_rx = update_rx.clone();
    // --- ON CLONE LA CONFIG POUR LE SCOUT ---
    let scout_config = config.clone();
    tokio::spawn(async move {
        // --- ON PASSE LA CONFIG ICI ---
        scout_worker(scout_graph, scout_rx, opportunity_tx, scout_config).await;
    });

    // --- Tâche 5 : Workers d'analyse (VERSION CORRIGÉE ET SIMPLIFIÉE) ---
    println!("[Init] Démarrage de {} workers d'analyse...", ANALYSIS_WORKER_COUNT);

    let template_cache = Arc::new(RwLock::new(HashMap::new()));

    // 1. On crée UN SEUL groupe de dépendances.
    let worker_deps = WorkerDependencies {
        rpc_client: rpc_client.clone(),
        slot_tracker: slot_tracker.clone(),
        slot_metronome: slot_metronome.clone(),
        leader_schedule_tracker: leader_schedule_tracker.clone(),
        validator_intel: validator_intel_service.clone(),
        fee_manager: fee_manager.clone(),
        sender: sender.clone(),
        config: config.clone(),
        template_cache: template_cache.clone(),
    };

    let blacklist_manager = BlacklistManager::load();
    let circuit_breaker_state = Arc::new(RwLock::new(HashMap::<String, FailureTracker>::new()));

    for i in 0..ANALYSIS_WORKER_COUNT {
        // 2. On clone les dépendances pour chaque worker. C'est une opération légère.
        let deps_for_worker = worker_deps.clone();

        let worker_rx = shared_opportunity_rx.clone();
        let worker_graph = hot_graph.clone();
        let worker_payer = Keypair::try_from(payer.to_bytes().as_slice())?;

        let breaker_clone = circuit_breaker_state.clone();
        let blacklist_clone = blacklist_manager.clone();

        tokio::spawn(async move {
            analysis_worker(
                i + 1,
                worker_rx,
                worker_graph,
                worker_payer,
                deps_for_worker, // On déplace la copie dans le worker
                breaker_clone,
                blacklist_clone,
            )
                .await;
        });
    }

    println!("[Bot] Architecture démarrée. Le bot est opérationnel.");
    std::future::pending::<()>().await;

    Ok(())
}