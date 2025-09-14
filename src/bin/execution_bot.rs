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
    time::Duration,
};
use tokio::sync::{Mutex, watch}; // <-- NOUVEL IMPORT
use yellowstone_grpc_client::GeyserGrpcClient;
use yellowstone_grpc_proto::prelude::{
    subscribe_update::UpdateOneof, CommitmentLevel, SubscribeRequest,
    SubscribeRequestFilterAccounts,
};
use spl_associated_token_account::instruction::create_associated_token_account;
use solana_sdk::transaction::VersionedTransaction;


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

// --- NOUVELLE ARCHITECTURE : LE CONSOMMATEUR (WORKER D'ANALYSE) ---
async fn analysis_worker(
    id: usize,
    shared_graph: Arc<ArcSwap<Graph>>,
    mut update_receiver: watch::Receiver<()>,
    rpc_client: Arc<ResilientRpcClient>,
    payer: Keypair,
    currently_processing: Arc<Mutex<HashSet<String>>>,
    slot_tracker: Arc<SlotTracker>,
    slot_metronome: Arc<SlotMetronome>,
    leader_schedule_tracker: Arc<LeaderScheduleTracker>,
    validator_intel: Arc<ValidatorIntelService>,
    fee_manager: FeeManager,
) {
    println!("[Worker {}] Démarrage.", id);
    loop {
        // Attendre une notification de mise à jour du graphe
        if update_receiver.changed().await.is_err() {
            println!("[Worker {}] Le canal de notification est fermé. Arrêt.", id);
            break;
        }

        let graph_snapshot = shared_graph.load_full();
        if graph_snapshot.pools.is_empty() { continue; }

        let opportunities = find_spatial_arbitrage(graph_snapshot.clone()).await;

        if let Some(opp) = opportunities.into_iter().next() {
            let mut pools = [opp.pool_buy_from_key.to_string(), opp.pool_sell_to_key.to_string()];
            pools.sort();
            let opportunity_id = format!("{}-{}", pools[0], pools[1]);

            let is_already_processing = {
                let mut processing_guard = currently_processing.lock().await;
                if processing_guard.contains(&opportunity_id) { true } else { processing_guard.insert(opportunity_id.clone()); false }
            };

            if !is_already_processing {
                // Pour la ré-hydratation, on prend une copie MUTABLE du pool depuis notre snapshot
                let pool_buy_from_data = graph_snapshot.pools.get(&opp.pool_buy_from_key).unwrap().clone();
                let pool_sell_to_data = graph_snapshot.pools.get(&opp.pool_sell_to_key).unwrap().clone();
                let rehydrated_opp = opp.clone();

                // On doit ré-hydrater les CLMMs si nécessaire avant de traiter
                let rehydrated_buy = rehydrate_if_clmm(pool_buy_from_data, &rpc_client).await;
                let rehydrated_sell = rehydrate_if_clmm(pool_sell_to_data, &rpc_client).await;

                if let (Ok(Some(new_buy)), Ok(Some(new_sell))) = (rehydrated_buy, rehydrated_sell) {
                    let mut temp_graph_for_processing = Graph::new();
                    temp_graph_for_processing.add_pool_to_graph(new_buy);
                    temp_graph_for_processing.add_pool_to_graph(new_sell);

                    if let Err(e) = process_opportunity(
                        rehydrated_opp, Arc::new(temp_graph_for_processing), rpc_client.clone(), payer.insecure_clone(), &fee_manager,
                        slot_tracker.clone(), slot_metronome.clone(), leader_schedule_tracker.clone(), validator_intel.clone()
                    ).await {
                        println!("[Worker {}] Erreur Traitement: {}", id, e);
                    }

                } else if let Err(e) = process_opportunity(
                    opp, graph_snapshot.clone(), rpc_client.clone(), payer.insecure_clone(), &fee_manager,
                    slot_tracker.clone(), slot_metronome.clone(), leader_schedule_tracker.clone(), validator_intel.clone()
                ).await {
                    println!("[Worker {}] Erreur Traitement: {}", id, e);
                }


                currently_processing.lock().await.remove(&opportunity_id);
            }
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


// MODIFIÉ : la fonction prend `Arc<Graph>` car le snapshot est immuable
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
) -> Result<()> {

    // --- PHASE 1: Estimation Locale (INCHANGÉE) ---
    println!("\n--- [Phase 1] Estimation locale de l'opportunité ---");
    let pool_buy_from = graph.pools.get(&opportunity.pool_buy_from_key).context("Pool (buy) non trouvé")?.clone();
    let pool_sell_to = graph.pools.get(&opportunity.pool_sell_to_key).context("Pool (sell) non trouvé")?.clone();
    let current_timestamp = slot_tracker.current().clock.unix_timestamp;
    let quote_buy_details = pool_buy_from.get_quote_with_details(&opportunity.token_in_mint, opportunity.amount_in, current_timestamp)?;
    let quote_sell_details = pool_sell_to.get_quote_with_details(&opportunity.token_intermediate_mint, quote_buy_details.amount_out, current_timestamp)?;
    let profit_brut_estime = quote_sell_details.amount_out.saturating_sub(opportunity.amount_in);
    if profit_brut_estime < 50000 { return Ok(()); }
    println!("     -> Profit Brut Estimé (local) : {} lamports", profit_brut_estime);
    let estimated_cus = cu_manager::estimate_arbitrage_cost(&pool_buy_from, quote_buy_details.ticks_crossed, &pool_sell_to, quote_sell_details.ticks_crossed);
    println!("     -> Coût estimé en Compute Units: {}", estimated_cus);

    // --- PHASE 2: Calcul des Protections (INCHANGÉE) ---
    println!("\n--- [Phase 2] Calcul des protections ---");
    let protections = protections::calculate_slippage_protections(
        opportunity.amount_in,
        profit_brut_estime,
        pool_sell_to.clone(),
        &opportunity.token_intermediate_mint,
        current_timestamp,
    )?;

    // --- PHASE 3: Analyse de Routage et Décision (INCHANGÉE) ---
    println!("\n--- [Phase 3] Analyse de routage et décision d'envoi ---");

    let managed_lut_address = Pubkey::from_str(MANAGED_LUT_ADDRESS)?;
    let lut_account_data = rpc_client.get_account_data(&managed_lut_address).await
        .context(format!("Le compte de la LUT ({}) n'a pas été trouvé.", MANAGED_LUT_ADDRESS))?;
    let lut_ref = AddressLookupTable::deserialize(&lut_account_data)?;
    let owned_lookup_table = SdkAddressLookupTableAccount {
        key: managed_lut_address,
        addresses: lut_ref.addresses.to_vec(),
    };

    let current_slot = slot_tracker.current().clock.slot;
    let time_remaining_in_slot = slot_metronome.estimated_time_remaining_in_slot_ms();
    let (target_slot, leader_identity) = if time_remaining_in_slot > BOT_PROCESSING_TIME_MS {
        (current_slot, leader_schedule_tracker.get_leader_for_slot(current_slot))
    } else {
        let next_slot = current_slot + 1;
        (next_slot, leader_schedule_tracker.get_leader_for_slot(next_slot))
    };
    let leader_identity = leader_identity.context(format!("Leader introuvable pour le slot {}", target_slot))?;

    // On prépare les variables pour la construction de la transaction finale
    let mut final_tx: Option<VersionedTransaction> = None;

    if let Some(_validator_info) = validator_intel.get_validator_info(&leader_identity).await {
        println!("     -> Leader cible (Slot {}): {} (✅ Jito)", target_slot, leader_identity);
        let jito_tip = fee_manager.calculate_jito_tip(profit_brut_estime, JITO_TIP_PERCENT, estimated_cus);
        let profit_net_final = profit_brut_estime.saturating_sub(jito_tip);

        if profit_net_final > 5000 {
            println!("  -> DÉCISION : PRÉPARER BUNDLE JITO. Profit net estimé: {} lamports", profit_net_final);
            // <-- MODIFIÉ : On passe bien owned_lookup_table
            let (tx, _) = transaction_builder::build_arbitrage_transaction(
                &opportunity, graph, &rpc_client, &payer, &owned_lookup_table, &protections, estimated_cus, 0
            ).await?;
            final_tx = Some(tx);
        } else {
            println!("  -> DÉCISION : Abandon. Profit Jito insuffisant.");
        }
    } else {
        println!("     -> Leader cible (Slot {}): {} (❌ Non-Jito)", target_slot, leader_identity);
        const OVERBID_PERCENT: u8 = 20;
        let accounts_in_tx = vec![opportunity.pool_buy_from_key, opportunity.pool_sell_to_key];
        let priority_fee_price_per_cu = fee_manager.calculate_priority_fee(&accounts_in_tx, OVERBID_PERCENT).await;
        let total_priority_fee = (estimated_cus * priority_fee_price_per_cu) / 1_000_000;
        let profit_net_final = profit_brut_estime.saturating_sub(5000).saturating_sub(total_priority_fee);

        if profit_net_final > 5000 {
            println!("  -> DÉCISION: PRÉPARER TRANSACTION NORMALE. Profit net estimé: {} lamports", profit_net_final);
            // <-- MODIFIÉ : On passe bien owned_lookup_table
            let (tx, _) = transaction_builder::build_arbitrage_transaction(
                &opportunity, graph, &rpc_client, &payer, &owned_lookup_table, &protections, estimated_cus, priority_fee_price_per_cu
            ).await?;
            final_tx = Some(tx);
        } else {
            println!("  -> DÉCISION : Abandon. Profit normal insuffisant.");
        }
    }

    // --- PHASE 4: Simulation de Vérification (NOUVEAU BLOC) ---
    if let Some(tx_to_simulate) = final_tx {
        println!("\n--- [Phase 4] Lancement de la simulation de vérification finale ---");

        let sim_config = RpcSimulateTransactionConfig {
            sig_verify: false,
            replace_recent_blockhash: true,
            commitment: Some(rpc_client.commitment()),
            encoding: Some(UiTransactionEncoding::Base64),
            ..Default::default()
        };

        match rpc_client.simulate_transaction_with_config(&tx_to_simulate, sim_config).await {
            Ok(sim_response) => {
                let sim_value = sim_response.value;
                if let Some(err) = sim_value.err {
                    println!("  -> ❌ ÉCHEC DE LA SIMULATION FINALE : {:?}", err);
                    if let Some(logs) = sim_value.logs {
                        println!("     Logs d'erreur :");
                        logs.iter().for_each(|log| println!("       {}", log));
                    }
                } else {
                    let real_profit = simulator::parse_profit_from_logs(&sim_value.logs.unwrap_or_default())
                        .unwrap_or(0);
                    println!("  -> ✅ SUCCÈS DE LA SIMULATION FINALE !");
                    println!("     -> Profit réel simulé : {} lamports", real_profit);
                    println!("     -> (L'envoi de la transaction se ferait ici)");
                }
            },
            Err(e) => {
                println!("  -> ❌ ERREUR RPC pendant la simulation finale : {}", e);
            }
        }
    }

    Ok(())
}


#[tokio::main]
async fn main() -> Result<()> {
    println!("--- Lancement de l'Execution Bot (Architecture Producteur-Consommateur) ---");
    dotenvy::dotenv().ok();

    // --- Initialisation (inchangée) ---
    let config = Config::load()?;
    let payer = Keypair::from_base58_string(&config.payer_private_key);
    let rpc_client = Arc::new(ResilientRpcClient::new(config.solana_rpc_url.clone(), 3, 500));
    println!("[Init] Le bot utilisera la LUT gérée à l'adresse: {}", MANAGED_LUT_ADDRESS);
    let geyser_url = env::var("GEYSER_GRPC_URL").context("GEYSER_GRPC_URL requis")?;
    let validators_app_token = env::var("VALIDATORS_APP_API_KEY").context("VALIDATORS_APP_API_KEY requis")?;
    // AFFICHER l'adresse de la LUT au démarrage pour le débogage
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
    let currently_processing = Arc::new(Mutex::new(HashSet::<String>::new()));

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

    // Tâche 4 : Les Workers d'analyse (Consommateurs)
    println!("[Init] Démarrage de {} workers d'analyse...", ANALYSIS_WORKER_COUNT);
    for i in 0..ANALYSIS_WORKER_COUNT {
        let worker_graph = hot_graph.clone();
        let worker_rx = update_rx.clone();
        let worker_rpc = rpc_client.clone();
        let worker_payer = Keypair::try_from(payer.to_bytes().as_slice())?;
        let worker_processing = currently_processing.clone();
        let worker_slot_tracker = slot_tracker.clone();
        let worker_slot_metronome = slot_metronome.clone();
        let worker_leader_schedule = leader_schedule_tracker.clone();
        let worker_validator_intel = validator_intel_service.clone();
        let worker_fee_manager = fee_manager.clone();

        tokio::spawn(async move {
            analysis_worker(
                i + 1, worker_graph, worker_rx, worker_rpc, worker_payer,
                worker_processing, worker_slot_tracker, worker_slot_metronome,
                worker_leader_schedule, worker_validator_intel, worker_fee_manager,
            ).await;
        });
    }

    // Le thread principal peut maintenant soit attendre, soit effectuer d'autres tâches (ex: monitoring)
    println!("[Bot] Architecture démarrée. Le bot est opérationnel.");
    std::future::pending::<()>().await;

    Ok(())
}