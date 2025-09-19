// DANS : src/bin/arbitrage_engine.rs

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

use anyhow::{Context, Result};
use arc_swap::ArcSwap;
use spl_associated_token_account::instruction::create_associated_token_account;
use solana_sdk::{
    pubkey::Pubkey, signature::Keypair, signer::Signer, transaction::Transaction,
    transaction::VersionedTransaction,
};
use std::str::FromStr;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use mev::decoders::PoolFactory;
use mev::state::slot_tracker::SlotState;
use zmq;
use mev::communication::{
    GatewayCommand, GeyserUpdate, ZmqTopic, ZMQ_COMMAND_ENDPOINT, ZMQ_DATA_ENDPOINT,
};
use mev::execution::confirmation_service::ConfirmationService;
use mev::state::balance_manager;
use tracing::warn;
use std::{
    collections::{HashMap, HashSet},
    env, fs,
    sync::{Arc, RwLock},
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, Mutex, watch};
use tracing::{error, info}; // <-- MODIFIÉ : Assurez-vous que `info` est bien importé

use mev::{
    config::Config,
    decoders::{Pool, PoolOperations, meteora, orca, raydium},
    execution::{
        cu_manager,
        fee_manager::FeeManager,
        sender::{TransactionSender, UnifiedSender},
        transaction_builder::ArbitrageInstructionsTemplate,
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


// Les fonctions helpers (read_hotlist, load_main_graph_from_cache, etc.) restent les mêmes.
// ... (copiez-collez ici les fonctions read_hotlist, load_main_graph_from_cache, ensure_pump_user_account_exists, ensure_atas_exist_for_pool)
// NOTE : `load_main_graph_from_cache` doit maintenant retourner `Arc<Graph>` où `Graph` n'a plus de verrous internes.
fn read_hotlist() -> Result<HashSet<Pubkey>> {
    let data = fs::read_to_string("hotlist.json")?;
    Ok(serde_json::from_str(&data)?)
}


async fn ensure_pump_user_account_exists(rpc_client: &Arc<ResilientRpcClient>, payer: &Keypair) -> Result<()> {
    info!(payer = %payer.pubkey(), "[Pré-vérification] Vérification du compte de volume utilisateur pump.fun...");
    let (pda, _) = Pubkey::find_program_address(&[b"user_volume_accumulator", payer.pubkey().as_ref()], &mev::decoders::pump::amm::PUMP_PROGRAM_ID);
    if rpc_client.get_account(&pda).await.is_err() {
        info!(pda = %pda, "Compte de volume pump.fun non trouvé. Création en cours...");
        let init_ix = mev::decoders::pump::amm::pool::create_init_user_volume_accumulator_instruction(&payer.pubkey())?;
        let recent_blockhash = rpc_client.get_latest_blockhash().await?;
        let transaction = Transaction::new_signed_with_payer(&[init_ix], Some(&payer.pubkey()), &[payer], recent_blockhash);
        let versioned_tx = VersionedTransaction::from(transaction);
        let signature = rpc_client.send_and_confirm_transaction(&versioned_tx).await?;
        info!(signature = %signature, "✅ Compte de volume pump.fun créé avec succès.");
    } else {
        info!(pda = %pda, "Compte de volume pump.fun déjà existant.");
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
        info!(pool_address = %pool.address(), ata_count = instructions_to_execute.len(), "Envoi de la transaction pour créer des ATAs...");
        let recent_blockhash = rpc_client.get_latest_blockhash().await?;
        let transaction = Transaction::new_signed_with_payer(&instructions_to_execute, Some(&payer.pubkey()), &[payer], recent_blockhash);
        let versioned_tx = VersionedTransaction::from(transaction);
        let signature = rpc_client.send_and_confirm_transaction(&versioned_tx).await?;
        info!(signature = %signature, pool_address = %pool.address(), "✅ ATA(s) créé(s) avec succès.");
    }
    Ok(())
}

// Le reste du fichier reste identique
// ...
// (collez le reste du fichier `arbitrage_engine.rs` ici, de la ligne `const BLACKLIST_FILE_NAME...` jusqu'à la fin)
// ...
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

#[derive(serde::Deserialize)]
struct PoolDataFromJson {
    owner: String,
    data_b64: String,
}

async fn hotlist_updater(
    shared_graph: Arc<ArcSwap<Graph>>,
    shared_hotlist: Arc<tokio::sync::RwLock<HashSet<Pubkey>>>,
    pool_factory: PoolFactory,
    payer: Keypair,
    rpc_client: Arc<ResilientRpcClient>,
    update_notifier: tokio::sync::watch::Sender<()>,
) {
    info!("[HotlistUpdater] Démarrage du service de mise à jour du graphe actif.");
    let mut interval = tokio::time::interval(Duration::from_secs(2));

    // Charger l'univers une seule fois pour des recherches rapides
    let universe_on_disk: HashMap<String, PoolDataFromJson> =
        fs::read_to_string("pools_universe.json")
            .ok()
            .and_then(|content| serde_json::from_str(&content).ok())
            .unwrap_or_else(HashMap::new);

    if universe_on_disk.is_empty() {
        error!("[HotlistUpdater] CRITICAL: pools_universe.json est vide ou illisible. Le bot ne pourra pas ajouter de nouveaux pools.");
    }

    loop {
        interval.tick().await;

        let new_hotlist_on_disk = match read_hotlist() {
            Ok(list) => list,
            Err(_) => continue, // Si le fichier est en cours d'écriture, on attend
        };

        let mut graph_changed = false;
        let mut graph_clone = (*shared_graph.load_full()).clone();

        // Étape 1: Ajouter les nouveaux pools
        for pool_address in &new_hotlist_on_disk {
            if !graph_clone.pools.contains_key(pool_address) {
                info!("[HotlistUpdater] Nouveau pool détecté dans la hotlist: {}", pool_address);
                if let Some(pool_json_data) = universe_on_disk.get(&pool_address.to_string()) {
                    if let (Ok(owner), Ok(data)) = (
                        Pubkey::from_str(&pool_json_data.owner),
                        STANDARD.decode(&pool_json_data.data_b64)
                    ) {
                        if let Ok(raw_pool) = pool_factory.decode_raw_pool(pool_address, &data, &owner) {
                            match Graph::hydrate_pool(raw_pool, &rpc_client).await {
                                Ok(mut hydrated_pool) => {
                                    if let Err(e) = ensure_atas_exist_for_pool(&rpc_client, &payer, &hydrated_pool).await {
                                        warn!("[Onboarding] Échec création ATA pour {}: {}", pool_address, e);
                                        continue;
                                    }
                                    match &mut hydrated_pool {
                                        Pool::RaydiumClmm(p) => p.populate_quote_lookup_table(true),
                                        Pool::OrcaWhirlpool(p) => p.populate_quote_lookup_table(true),
                                        Pool::MeteoraDlmm(p) => p.populate_quote_lookup_table(true),
                                        Pool::MeteoraDammV2(p) => p.populate_quote_lookup_table(true),
                                        _ => {}
                                    }
                                    graph_clone.add_pool_to_graph(hydrated_pool);
                                    graph_changed = true;
                                },
                                Err(e) => warn!(pool = %pool_address, error = %e, "Échec hydratation du nouveau pool."),
                            }
                        }
                    }
                } else {
                    warn!("[HotlistUpdater] Le pool {} est dans la hotlist mais pas dans l'univers!", pool_address);
                }
            }
        }

        // Étape 2: Retirer les anciens pools
        let current_pools_in_graph: HashSet<Pubkey> = graph_clone.pools.keys().cloned().collect();
        for pool_to_remove in current_pools_in_graph.difference(&new_hotlist_on_disk) {
            info!("[HotlistUpdater] Retrait du pool inactif du graphe: {}", pool_to_remove);
            graph_clone.pools.remove(pool_to_remove);
            // On pourrait aussi nettoyer la account_to_pool_map si nécessaire, mais c'est moins critique
            graph_changed = true;
        }

        // Étape 3: Mettre à jour les structures partagées si nécessaire
        if graph_changed {
            shared_graph.store(Arc::new(graph_clone));
            let _ = update_notifier.send(());
        }

        // On met à jour la hotlist partagée pour le command_manager
        let mut writer = shared_hotlist.write().await;
        if *writer != new_hotlist_on_disk {
            *writer = new_hotlist_on_disk;
        }
    }
}

async fn clmm_pre_hydrator(
    shared_graph: Arc<ArcSwap<Graph>>,
    rpc_client: Arc<ResilientRpcClient>,
    update_notifier: watch::Sender<()>,
) {
    info!("[Hydrator] Démarrage du service de pré-hydratation des CLMMs.");
    let mut interval = tokio::time::interval(Duration::from_secs(2));
    loop {
        interval.tick().await;
        let old_graph = shared_graph.load_full();
        if old_graph.pools.is_empty() { continue; }

        let mut new_graph = (*old_graph).clone();
        let mut graph_was_updated = false;
        let mut hydration_futures = Vec::new();

        for (pool_key, pool) in new_graph.pools.iter() {
            if matches!(pool, Pool::RaydiumClmm(_) | Pool::OrcaWhirlpool(_) | Pool::MeteoraDlmm(_) | Pool::MeteoraDammV2(_)) {
                let pool_clone = pool.clone();
                let rpc_clone = rpc_client.clone();
                let key_clone = *pool_key;

                hydration_futures.push(tokio::spawn(async move {
                    let result: Result<Pool> = async {
                        match pool_clone {
                            Pool::RaydiumClmm(mut p) => { raydium::clmm::hydrate(&mut p, &rpc_clone).await?; Ok(Pool::RaydiumClmm(p)) },
                            Pool::OrcaWhirlpool(mut p) => { orca::whirlpool::hydrate(&mut p, &rpc_clone).await?; Ok(Pool::OrcaWhirlpool(p)) },
                            Pool::MeteoraDlmm(mut p) => { meteora::dlmm::hydrate(&mut p, &rpc_clone).await?; Ok(Pool::MeteoraDlmm(p)) },
                            Pool::MeteoraDammV2(mut p) => { meteora::damm_v2::hydrate(&mut p, &rpc_clone).await?; Ok(Pool::MeteoraDammV2(p)) }
                            _ => unreachable!(),
                        }
                    }.await;
                    (key_clone, result)
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
            shared_graph.store(Arc::new(new_graph));
            let _ = update_notifier.send(());
        }
    }
}

// <-- NOUVEAU: Les tâches de fond de la nouvelle architecture -->
async fn data_listener(
    shared_graph: Arc<ArcSwap<Graph>>,
    slot_tracker: Arc<SlotTracker>, // <-- MODIFIÉ: On reçoit le SlotTracker
    update_notifier: watch::Sender<()>,
) -> Result<()> {
    info!("[Engine/Listener] Démarrage du listener de données ZMQ.");
    let context = zmq::Context::new();
    let subscriber = context.socket(zmq::SUB)?;
    subscriber.connect(ZMQ_DATA_ENDPOINT)?;
    subscriber.set_subscribe(&bincode::serialize(&ZmqTopic::Slot)?)?;
    subscriber.set_subscribe(&bincode::serialize(&ZmqTopic::Account)?)?;

    loop {
        let multipart = subscriber.recv_multipart(0)?;
        if multipart.len() != 2 { continue; }
        let update: GeyserUpdate = bincode::deserialize(&multipart[1])?;

        match update {
            GeyserUpdate::Slot(simple_slot_update) => {
                let new_state = SlotState {
                    clock: solana_sdk::clock::Clock {
                        slot: simple_slot_update.slot,
                        // On met des valeurs par défaut, car seul `slot` et `received_at` comptent.
                        epoch_start_timestamp: 0,
                        epoch: 0,
                        leader_schedule_epoch: 0,
                        unix_timestamp: 0,
                    },
                    received_at: Instant::now(), // Le plus important : l'heure de réception
                };
                slot_tracker.update_from_geyser(new_state);
            }
            GeyserUpdate::Account(simple_account_update) => {
                let account_key = simple_account_update.pubkey;
                let data = simple_account_update.data;

                let old_graph = shared_graph.load_full();
                if let Some(pool_addr) = old_graph.account_to_pool_map.get(&account_key) {
                    let mut new_graph = (*old_graph).clone();
                    if let Some(pool) = new_graph.pools.get_mut(pool_addr) {
                        if pool.update_from_account_data(&account_key, &data).is_ok() {
                            shared_graph.store(Arc::new(new_graph));
                            let _ = update_notifier.send(());
                        }
                    }
                }
            }
            _ => {}
        }
    }
}
async fn command_manager(hotlist_reader: Arc<tokio::sync::RwLock<HashSet<Pubkey>>>, graph_reader: Arc<ArcSwap<Graph>>) -> Result<()> {
    info!("[Engine/Commander] Démarrage du gestionnaire de commandes ZMQ.");
    let context = zmq::Context::new();
    let commander = context.socket(zmq::PUSH)?;
    commander.connect(ZMQ_COMMAND_ENDPOINT)?;
    let mut last_sent_hotlist = HashSet::new();

    loop {
        tokio::time::sleep(Duration::from_secs(5)).await;
        let hotlist_snapshot = hotlist_reader.read().await.clone();
        if hotlist_snapshot != last_sent_hotlist {
            info!("[Engine/Commander] Hotlist modifiée. Envoi de la commande de mise à jour au Gateway.");
            let graph = graph_reader.load_full();
            let accounts_to_watch: Vec<Pubkey> = hotlist_snapshot.iter()
                .filter_map(|key| graph.pools.get(key))
                .flat_map(|pool| match pool {
                    Pool::RaydiumClmm(_) | Pool::OrcaWhirlpool(_) | Pool::MeteoraDlmm(_) | Pool::MeteoraDammV2(_) => vec![pool.address()],
                    _ => { let (v_a, v_b) = pool.get_vaults(); vec![v_a, v_b] }
                }).collect();

            let command = GatewayCommand::UpdateAccountSubscriptions(accounts_to_watch);
            commander.send(bincode::serialize(&command)?, 0)?;
            last_sent_hotlist = hotlist_snapshot;
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

            if tracker.consecutive_failures >= deps.config.circuit_breaker_failure_threshold { // <--- UTILISEZ LA CONFIG
                tracker.cooldown_trigger_count += 1;
                warn!(
                    "[Worker {}] DISJONCTEUR DÉCLENCHÉ ({}ème fois) pour la paire {} ! Mise en pause pour {} secondes.",
                    id, tracker.cooldown_trigger_count, pool_pair_id, deps.config.circuit_breaker_cooldown_secs // <--- UTILISEZ LA CONFIG
                );
                tracker.cooldown_until = Some(Instant::now() + Duration::from_secs(deps.config.circuit_breaker_cooldown_secs)); // <--- UTILISEZ LA CONFIG
                metrics::CIRCUIT_BREAKER_TRIPPED.with_label_values(&[&pool_pair_id]).inc();

                if tracker.cooldown_trigger_count >= deps.config.circuit_breaker_blacklist_threshold { // <--- UTILISEZ LA CONFIG
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
    mev::monitoring::logging::setup_logging();
    dotenvy::dotenv().ok();
    info!("--- Démarrage de l'Arbitrage Engine (Architecture Sélective) ---");

    // --- 1. Initialisation des Services de Base ---
    let config = Config::load()?;
    if config.dry_run {
        warn!("--- LE BOT EST EN MODE DRY RUN. AUCUNE TRANSACTION REELLE NE SERA ENVOYEE. ---");
    } else {
        info!("--- Le bot est en mode LIVE. Les transactions seront envoyées sur la blockchain. ---");
    }
    let payer = Keypair::from_base58_string(&config.payer_private_key);
    let rpc_client = Arc::new(ResilientRpcClient::new(config.solana_rpc_url.clone(), 3, 500));
    let pool_factory = PoolFactory::new(rpc_client.clone());

    let (confirmation_service, tx_to_confirm_sender) = ConfirmationService::new(rpc_client.clone());
    confirmation_service.start();
    let sender: Arc<dyn TransactionSender> = Arc::new(UnifiedSender::new(rpc_client.clone(), config.dry_run, tx_to_confirm_sender));

    let validators_app_token = env::var("VALIDATORS_APP_API_KEY").context("VALIDATORS_APP_API_KEY requis")?;
    let validator_intel_service = Arc::new(ValidatorIntelService::new(validators_app_token.clone()).await?);
    validator_intel_service.start(validators_app_token);

    let slot_tracker = Arc::new(SlotTracker::new(&rpc_client).await?);
    let leader_schedule_tracker = Arc::new(LeaderScheduleTracker::new(rpc_client.clone()).await?);
    leader_schedule_tracker.start();
    let slot_metronome = Arc::new(SlotMetronome::new(slot_tracker.clone()));
    slot_metronome.start();

    balance_manager::start_balance_manager(rpc_client.clone(), payer.insecure_clone(), config.clone());
    balance_tracker::start_monitoring(rpc_client.clone(), config.clone());
    ensure_pump_user_account_exists(&rpc_client, &payer).await?;

    // --- 2. Construction du Graphe Initial (0 appel RPC) ---
    info!("[Main] Démarrage du moteur avec une stratégie de chargement sélectif...");
    let initial_hotlist = read_hotlist().unwrap_or_else(|_| {
        warn!("hotlist.json non trouvée au démarrage, démarrage avec un graphe vide.");
        HashSet::new()
    });

    let universe_on_disk: HashMap<String, PoolDataFromJson> = fs::read_to_string("pools_universe.json")
        .ok()
        .and_then(|content| serde_json::from_str(&content).ok())
        .unwrap_or_else(HashMap::new);

    let mut initial_graph = Graph::new();
    info!("[Main] Peuplement du graphe initial avec {} pools de la hotlist...", initial_hotlist.len());
    for pool_address in initial_hotlist {
        if let Some(pool_json_data) = universe_on_disk.get(&pool_address.to_string()) {
            if let (Ok(owner), Ok(data)) = (
                Pubkey::from_str(&pool_json_data.owner),
                STANDARD.decode(&pool_json_data.data_b64)
            ) {
                if let Ok(raw_pool) = pool_factory.decode_raw_pool(&pool_address, &data, &owner) {
                    match Graph::hydrate_pool(raw_pool, &rpc_client).await {
                        Ok(mut hydrated_pool) => {
                            if let Err(e) = ensure_atas_exist_for_pool(&rpc_client, &payer, &hydrated_pool).await {
                                warn!("[Onboarding] Échec création ATA pour {}: {}", pool_address, e);
                                continue;
                            }
                            match &mut hydrated_pool {
                                Pool::RaydiumClmm(p) => p.populate_quote_lookup_table(true),
                                Pool::OrcaWhirlpool(p) => p.populate_quote_lookup_table(true),
                                Pool::MeteoraDlmm(p) => p.populate_quote_lookup_table(true),
                                Pool::MeteoraDammV2(p) => p.populate_quote_lookup_table(true),
                                _ => {}
                            }
                            initial_graph.add_pool_to_graph(hydrated_pool);
                        },
                        Err(e) => warn!(pool = %pool_address, error = %e, "Échec hydratation initiale."),
                    }
                }
            }
        }
    }
    info!("[Main] Graphe initial prêt avec {} pools.", initial_graph.pools.len());

    // --- 3. Initialisation des Données Partagées ---
    let shared_graph = Arc::new(ArcSwap::new(Arc::new(initial_graph)));
    let (update_tx, update_rx) = watch::channel(());
    let (opportunity_tx, opportunity_rx) = mpsc::channel::<ArbitrageOpportunity>(1024);
    let shared_opportunity_rx = Arc::new(Mutex::new(opportunity_rx));

    // NOUVEAU: Hotlist partagée pour le command_manager
    let shared_hotlist = Arc::new(tokio::sync::RwLock::new(read_hotlist().unwrap_or_default()));

    // --- 4. Démarrage des Tâches de Fond ---
    info!("[Main] Démarrage des services de communication et de données...");
    tokio::spawn(data_listener(shared_graph.clone(), slot_tracker.clone(), update_tx.clone()));

    // NOUVEAU: On lance le hotlist_updater ici !
    tokio::spawn(hotlist_updater(
        shared_graph.clone(),
        shared_hotlist.clone(),
        pool_factory.clone(),
        payer.insecure_clone(),
        rpc_client.clone(),
        update_tx.clone()
    ));

    // Le command_manager utilise la shared_hotlist pour être réactif
    tokio::spawn(command_manager(shared_hotlist.clone(), shared_graph.clone()));

    // On conserve le pre_hydrator car vous le souhaitez pour la performance
    tokio::spawn(clmm_pre_hydrator(shared_graph.clone(), rpc_client.clone(), update_tx.clone()));

    info!("[Main] Démarrage des services de trading...");
    let fee_manager = FeeManager::new(rpc_client.clone());
    // Le fee_manager s'abonne aussi à la hotlist partagée
    fee_manager.start(shared_hotlist.clone());
    tokio::spawn(scout_worker(shared_graph.clone(), update_rx, opportunity_tx, config.clone()));

    info!("[Main] Démarrage des workers d'analyse...");
    let worker_deps = WorkerDependencies {
        rpc_client: rpc_client.clone(),
        slot_tracker: slot_tracker.clone(),
        slot_metronome: slot_metronome.clone(),
        leader_schedule_tracker: leader_schedule_tracker.clone(),
        validator_intel: validator_intel_service.clone(),
        fee_manager: fee_manager.clone(),
        sender: sender.clone(),
        config: config.clone(),
        template_cache: Arc::new(RwLock::new(HashMap::new())),
    };

    let blacklist_manager = BlacklistManager::load();
    let circuit_breaker_state = Arc::new(RwLock::new(HashMap::new()));

    for i in 0..config.analysis_worker_count {
        let deps_for_worker = worker_deps.clone();
        let worker_rx = shared_opportunity_rx.clone();
        let worker_graph = shared_graph.clone();
        let worker_payer = payer.insecure_clone();
        let breaker_clone = circuit_breaker_state.clone();
        let blacklist_clone = blacklist_manager.clone();
        tokio::spawn(async move {
            analysis_worker(i + 1, worker_rx, worker_graph, worker_payer, deps_for_worker, breaker_clone, blacklist_clone).await;
        });
    }

    info!("[Engine] Tous les services sont démarrés. L'Arbitrage Engine est opérationnel.");
    std::future::pending::<()>().await;

    Ok(())
}