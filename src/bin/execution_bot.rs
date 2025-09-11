use anyhow::{Result, Context};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use futures_util::StreamExt;
use mev::{
    config::Config,
    decoders::{Pool, PoolOperations, raydium::clmm as raydium_clmm, orca::whirlpool as orca_whirlpool, meteora::dlmm as meteora_dlmm},
    execution::{protections, simulator, transaction_builder},
    graph_engine::Graph,
    rpc::ResilientRpcClient,
    state::slot_tracker::SlotTracker,
    strategies::spatial::{find_spatial_arbitrage, ArbitrageOpportunity},
};
use solana_account_decoder::{UiAccountData, UiAccountEncoding};
use solana_address_lookup_table_program::state::AddressLookupTable;
use solana_client::{
    nonblocking::pubsub_client::PubsubClient, rpc_config::RpcAccountInfoConfig,
};
use solana_program_pack::Pack;
use solana_sdk::{
    message::AddressLookupTableAccount as SdkAddressLookupTableAccount, pubkey::Pubkey,
    signature::Keypair,
};
use spl_token::state::Account as SplTokenAccount;
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::Read,
    sync::{Arc, RwLock},
    time::Duration,
};
use tokio::sync::{mpsc, Mutex};



/// S'occupe de maintenir le hot_graph à jour avec les dernières données de liquidité
/// pour les pools complexes (CLMM, DLMM) en écoutant les changements sur leurs comptes de state.
struct PoolHydrator {
    hot_graph: Arc<Mutex<Graph>>,
    rpc_client: Arc<ResilientRpcClient>,
    pubsub_client: Arc<PubsubClient>,
    // Stocke le dernier tick/bin connu pour chaque pool afin de détecter la direction du mouvement
    previous_ticks: Arc<RwLock<HashMap<Pubkey, i32>>>,
    // Garde une trace des abonnements actifs pour ne pas dupliquer
    active_subscriptions: Arc<Mutex<HashSet<Pubkey>>>,
}

impl PoolHydrator {
    fn new(
        hot_graph: Arc<Mutex<Graph>>,
        rpc_client: Arc<ResilientRpcClient>,
        pubsub_client: Arc<PubsubClient>,
    ) -> Self {
        Self {
            hot_graph,
            rpc_client,
            pubsub_client,
            previous_ticks: Arc::new(RwLock::new(HashMap::new())),
            active_subscriptions: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    /// La boucle principale qui surveille la hotlist et lance/arrête les abonnements.
    async fn run(&self) {
        println!("[Hydrator] Démarrage du service d'hydratation en temps réel.");
        let mut interval = tokio::time::interval(Duration::from_secs(10)); // On vérifie la hotlist toutes les 10s

        loop {
            interval.tick().await;

            let hotlist_pools = match read_hotlist() {
                Ok(pools) => pools,
                Err(e) => {
                    eprintln!("[Hydrator] Erreur de lecture de la hotlist: {}", e);
                    continue;
                }
            };

            let mut active_subs = self.active_subscriptions.lock().await;

            // Abonnements pour les nouveaux pools
            for pool_address in &hotlist_pools {
                if !active_subs.contains(pool_address) {
                    println!("[Hydrator] Nouveau pool détecté dans la hotlist: {}. Lancement de l'abonnement.", pool_address);
                    active_subs.insert(*pool_address);
                    self.spawn_subscription_task(*pool_address);
                }
            }

            // (Optionnel) Ici, on pourrait ajouter une logique pour se désabonner des pools qui ne sont plus dans la hotlist.
        }
    }

    /// Lance une tâche dédiée pour s'abonner à un pool spécifique.
    fn spawn_subscription_task(&self, pool_address: Pubkey) {
        let hot_graph = self.hot_graph.clone();
        let rpc_client = self.rpc_client.clone();
        let pubsub_client = self.pubsub_client.clone();
        let previous_ticks = self.previous_ticks.clone();

        tokio::spawn(async move {
            if let Err(e) = Self::subscribe_and_hydrate_pool(hot_graph, rpc_client, pubsub_client, previous_ticks, pool_address).await {
                eprintln!("[Hydrator Task {}] Erreur: {}", pool_address, e);
            }
        });
    }

    /// La logique coeur : s'abonne, détecte la direction et met à jour le graphe.
    async fn subscribe_and_hydrate_pool(
        hot_graph: Arc<Mutex<Graph>>,
        rpc_client: Arc<ResilientRpcClient>,
        pubsub_client: Arc<PubsubClient>,
        previous_ticks: Arc<RwLock<HashMap<Pubkey, i32>>>,
        pool_address: Pubkey,
    ) -> Result<()> {
        let (mut stream, _) = pubsub_client.account_subscribe(
            &pool_address,
            Some(RpcAccountInfoConfig {
                encoding: Some(UiAccountEncoding::Base64),
                ..Default::default()
            }),
        ).await.context(format!("Failed to subscribe to account {}", pool_address))?;

        println!("[Hydrator Task {}] Abonnement au compte de state réussi.", pool_address);

        // Cette boucle est correcte. `response` est bien de type RpcResponse<UiAccount>.
        while let Some(response) = stream.next().await {
            // Pas de `match` ici. On traite directement la réponse.
            let ui_account = response.value;
            let data = match ui_account.data {
                UiAccountData::Binary(encoded_data, _) => match STANDARD.decode(encoded_data) {
                    Ok(d) => d,
                    Err(_) => continue,
                },
                _ => continue,
            };

            let mut graph = hot_graph.lock().await;

            let old_pool_arc = match graph.pools.get(&pool_address) {
                Some(p) => p.clone(),
                None => break,
            };

            let mut new_pool = (*old_pool_arc).clone();

            let (current_tick, is_clmm_type, program_id) = match &new_pool {
                Pool::RaydiumClmm(p) => (raydium_clmm::decode_pool(&p.address, &data, &p.program_id).map(|p| p.tick_current).unwrap_or(p.tick_current), true, p.program_id),
                Pool::OrcaWhirlpool(p) => (orca_whirlpool::decode_pool(&p.address, &data).map(|p| p.tick_current_index).unwrap_or(p.tick_current_index), true, p.program_id),
                Pool::MeteoraDlmm(p) => (meteora_dlmm::decode_lb_pair(&p.address, &data, &p.program_id).map(|p| p.active_bin_id).unwrap_or(p.active_bin_id), true, p.program_id),
                _ => (0, false, Pubkey::default()),
            };

            if !is_clmm_type { continue; }

            let last_known_tick = *previous_ticks.read().unwrap().get(&pool_address).unwrap_or(&0);
            if current_tick == last_known_tick { continue; }

            let go_up = current_tick > last_known_tick;
            previous_ticks.write().unwrap().insert(pool_address, current_tick);

            let decode_result = match &mut new_pool {
                Pool::RaydiumClmm(p) => raydium_clmm::decode_pool(&p.address, &data, &program_id).map(|d| *p = d),
                Pool::OrcaWhirlpool(p) => orca_whirlpool::decode_pool(&p.address, &data).map(|d| *p = d),
                Pool::MeteoraDlmm(p) => meteora_dlmm::decode_lb_pair(&p.address, &data, &program_id).map(|d| *p = d),
                _ => Ok(()),
            };

            if decode_result.is_err() { continue; }

            let rehydration_result = match &mut new_pool {
                Pool::RaydiumClmm(p) => raydium_clmm::hydrate(p, &rpc_client).await,
                Pool::OrcaWhirlpool(p) => orca_whirlpool::rehydrate_for_escalation(p, &rpc_client, go_up).await,
                Pool::MeteoraDlmm(p) => meteora_dlmm::rehydrate_for_escalation(p, &rpc_client, go_up).await,
                _ => Ok(()),
            };

            if rehydration_result.is_ok() {
                graph.pools.insert(pool_address, Arc::new(new_pool));
            } else {
                eprintln!("[Hydrator Task {}] Échec de la réhydratation pour le pool.", pool_address);
            }
        }

        // --- LA ROBUSTESSE EST AJOUTÉE ICI ---
        // Si on sort de la boucle, c'est que le stream s'est terminé.
        // On retourne une erreur pour que le code appelant sache que la tâche est morte.
        Err(anyhow::anyhow!("Le stream d'abonnement pour le pool {} s'est terminé de manière inattendue.", pool_address))
    }
}



fn load_main_graph_from_cache() -> Result<Graph> {
    println!("[Graph] Chargement du cache de pools de référence depuis 'graph_cache.bin'...");
    let mut file = std::fs::File::open("graph_cache.bin")?;
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer)?;
    let decoded_pools: HashMap<Pubkey, Pool> = bincode::deserialize(&buffer)?;
    let pools_with_arc: HashMap<Pubkey, Arc<Pool>> = decoded_pools
        .into_iter()
        .map(|(key, pool)| (key, Arc::new(pool)))
        .collect();
    Ok(Graph {
        pools: pools_with_arc,
        account_to_pool_map: HashMap::new(),
    })
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
) { // ... (le corps de cette fonction reste identique)
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
    // ... (le corps de cette fonction reste identique)
    // Elle ne mettra à jour que les pools de type AMM/CPMM, ce qui est correct.
    let mut graph_guard = match graph.try_lock() {
        Ok(guard) => guard,
        Err(_) => return,
    };
    let mut temp_map = HashMap::new();
    for (pool_addr, pool_arc) in &graph_guard.pools {
        let (v_a, v_b) = pool_arc.get_vaults();
        temp_map.insert(v_a, *pool_addr);
        temp_map.insert(v_b, *pool_addr);
    }
    if let Some(pool_address) = temp_map.get(vault_address) {
        if let Some(pool_arc) = graph_guard.pools.get(pool_address) {
            let mut pool_clone = (**pool_arc).clone();
            let (vault_a, _) = pool_clone.get_vaults();
            let is_vault_a = vault_address == &vault_a;
            let updated = match &mut pool_clone {
                Pool::RaydiumAmmV4(p) => { if is_vault_a { p.reserve_a = new_balance } else { p.reserve_b = new_balance }; true },
                Pool::RaydiumCpmm(p) => { if is_vault_a { p.reserve_a = new_balance } else { p.reserve_b = new_balance }; true },
                Pool::PumpAmm(p) => { if is_vault_a { p.reserve_a = new_balance } else { p.reserve_b = new_balance }; true },
                _ => false
            };
            if updated {
                graph_guard.pools.insert(*pool_address, Arc::new(pool_clone));
            }
        }
    }
}

const ADDRESS_LOOKUP_TABLE_ADDRESS: Pubkey = solana_sdk::pubkey!("E5h798UBdK8V1L7MvRfi1ppr2vitPUUUUCVqvTyDgKXN");

async fn process_opportunity(
    opportunity: ArbitrageOpportunity,
    graph: Arc<Mutex<Graph>>,
    rpc_client: Arc<ResilientRpcClient>,
    payer: Keypair,
    current_timestamp: i64,
) -> Result<()> {
    let lookup_table_account_data = rpc_client.get_account_data(&ADDRESS_LOOKUP_TABLE_ADDRESS).await?;
    let lookup_table_ref = AddressLookupTable::deserialize(&lookup_table_account_data)?;
    let owned_lookup_table = SdkAddressLookupTableAccount {
        key: ADDRESS_LOOKUP_TABLE_ADDRESS,
        addresses: lookup_table_ref.addresses.to_vec(),
    };

    let protections = {
        let pool_sell_to = {
            let graph_guard = graph.lock().await;
            (**graph_guard.pools.get(&opportunity.pool_sell_to_key).unwrap()).clone()
        };
        match protections::calculate_slippage_protections(
            opportunity.amount_in,
            opportunity.profit_in_lamports,
            pool_sell_to,
            &opportunity.token_intermediate_mint,
            current_timestamp,
        ) {
            Ok(p) => p,
            Err(e) => {
                println!("[Phase 1 ERREUR] Échec calcul protections : {}", e);
                return Ok(());
            }
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
    println!("--- Lancement de l'Execution Bot (Avec Hydrateur Proactif) ---");

    let config = Config::load()?;
    let payer = Keypair::from_base58_string(&config.payer_private_key);
    let rpc_client = Arc::new(ResilientRpcClient::new(config.solana_rpc_url.clone(), 3, 500));
    let main_graph = Arc::new(load_main_graph_from_cache()?); // Pas besoin de Mutex ici
    let hot_graph = Arc::new(Mutex::new(Graph::new()));
    let wss_url = config.solana_rpc_url.replace("http", "ws");
    let pubsub_client = Arc::new(PubsubClient::new(&wss_url).await?);
    let (update_sender, mut update_receiver) = mpsc::channel::<(Pubkey, u64)>(100);

    let slot_tracker = Arc::new(SlotTracker::new(&rpc_client).await?);
    slot_tracker.start(rpc_client.clone(), pubsub_client.clone());

    // NOUVEAU : Instancier et lancer l'Hydrateur en tâche de fond
    let hydrator = PoolHydrator::new(hot_graph.clone(), rpc_client.clone(), pubsub_client.clone());
    tokio::spawn(async move {
        hydrator.run().await;
    });

    let currently_processing = Arc::new(Mutex::new(HashSet::<String>::new()));

    // TÂCHE D'ADMISSION (code complet maintenant)
    let hot_graph_clone_for_onboarding = Arc::clone(&hot_graph);
    let main_graph_clone_for_onboarding = Arc::clone(&main_graph);
    let rpc_client_clone_for_onboarding = Arc::clone(&rpc_client);
    let pubsub_client_clone_for_vaults = Arc::clone(&pubsub_client);
    let update_sender_clone_for_vaults = update_sender.clone();

    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(5));
        loop {
            interval.tick().await;

            let hotlist = match read_hotlist() {
                Ok(h) => h,
                Err(_) => continue,
            };

            let mut graph = hot_graph_clone_for_onboarding.lock().await;

            for pool_address in hotlist {
                if graph.pools.contains_key(&pool_address) {
                    continue;
                }

                let unhydrated_pool_arc = match main_graph_clone_for_onboarding.pools.get(&pool_address) {
                    Some(p) => p.clone(),
                    None => continue,
                };

                let unhydrated_pool = (*unhydrated_pool_arc).clone();
                let hydrated_pool = match graph.hydrate_pool(unhydrated_pool, &rpc_client_clone_for_onboarding).await {
                    Ok(p) => p,
                    Err(_) => continue,
                };

                let (vault_a, vault_b) = hydrated_pool.get_vaults();
                graph.pools.insert(pool_address, Arc::new(hydrated_pool));

                tokio::spawn(subscribe_to_hot_vault(
                    pubsub_client_clone_for_vaults.clone(),
                    vault_a,
                    update_sender_clone_for_vaults.clone(),
                ));
                tokio::spawn(subscribe_to_hot_vault(
                    pubsub_client_clone_for_vaults.clone(),
                    vault_b,
                    update_sender_clone_for_vaults.clone(),
                ));
            }
        }
    });

    println!("[Bot] Prêt. En attente d'opportunités d'arbitrage...");
    loop {
        if let Some((vault_address, new_balance)) = update_receiver.recv().await {
            update_hot_graph_reserve(&hot_graph, &vault_address, new_balance);

            let graph_clone = Arc::clone(&hot_graph);
            let rpc_clone_for_processing = Arc::clone(&rpc_client); // CORRIGÉ
            let payer_clone_task = Keypair::try_from(payer.to_bytes().as_slice())?;
            let processing_clone = Arc::clone(&currently_processing);
            let tracker_clone = Arc::clone(&slot_tracker);

            tokio::spawn(async move {
                let clock_snapshot = tracker_clone.current();
                let current_timestamp = clock_snapshot.unix_timestamp;

                let opportunities = find_spatial_arbitrage(graph_clone.clone()).await;

                if let Some(opp) = opportunities.into_iter().next() {
                    let mut pools = [opp.pool_buy_from_key.to_string(), opp.pool_sell_to_key.to_string()];
                    pools.sort();
                    let opportunity_id = format!("{}-{}", pools[0], pools[1]);

                    let is_already_processing = {
                        let mut processing_guard = processing_clone.lock().await;
                        if processing_guard.contains(&opportunity_id) { true } else { processing_guard.insert(opportunity_id.clone()); false }
                    };

                    if !is_already_processing {
                        if let Err(e) = process_opportunity(opp, graph_clone, rpc_clone_for_processing, payer_clone_task, current_timestamp).await {
                            println!("[Erreur Traitement] {}", e);
                        }
                        let mut processing_guard = processing_clone.lock().await;
                        processing_guard.remove(&opportunity_id);
                    }
                }
            });
        }
    }
}