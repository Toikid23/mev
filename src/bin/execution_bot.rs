// DANS : src/bin/execution_bot.rs (Version Définitive)

use anyhow::Result;
use futures_util::StreamExt;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use solana_address_lookup_table_program::state::AddressLookupTable;
use solana_account_decoder::{UiAccountEncoding, UiAccountData};
use solana_client::{
    nonblocking::{pubsub_client::PubsubClient},
    rpc_config::RpcAccountInfoConfig,
};
use solana_program_pack::Pack;
use solana_sdk::{
    pubkey::Pubkey,
    signature::Keypair,
};
use solana_sdk::message::AddressLookupTableAccount as SdkAddressLookupTableAccount;
use spl_token::state::Account as SplTokenAccount;
use std::{collections::{HashMap, HashSet}, fs, io::Read, sync::Arc};
use tokio::{sync::{mpsc, Mutex}};
use mev::decoders::PoolOperations;

use mev::{
    config::Config,
    decoders::Pool,
    graph_engine::Graph,
    rpc::ResilientRpcClient,
    strategies::spatial::{find_spatial_arbitrage, ArbitrageOpportunity},
    execution::{
        transaction_builder,
        simulator,
        protections,
        // sender, // Gardé en commentaire
    },
};

fn load_main_graph_from_cache() -> Result<Graph> {
    println!("[Graph] Chargement du cache de pools de référence depuis 'graph_cache.bin'...");
    let mut file = std::fs::File::open("graph_cache.bin")?;
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer)?;

    let decoded_pools: HashMap<Pubkey, Pool> = bincode::deserialize(&buffer)?;

    // On convertit les Pool en Arc<Pool>
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
    let mut graph_guard = match graph.try_lock() {
        Ok(guard) => guard,
        Err(_) => return, // Si le graphe est déjà verrouillé, on abandonne pour ne pas bloquer.
    };

    // On a besoin d'une copie de la map pour trouver le pool_address sans garder un &graph_guard.pools
    let mut temp_map = HashMap::new();
    for (pool_addr, pool_arc) in &graph_guard.pools {
        let (v_a, v_b) = pool_arc.get_vaults();
        temp_map.insert(v_a, *pool_addr);
        temp_map.insert(v_b, *pool_addr);
    }

    if let Some(pool_address) = temp_map.get(vault_address) {
        // 1. On récupère le Arc<Pool> existant.
        if let Some(pool_arc) = graph_guard.pools.get(pool_address) {

            // 2. On clone les données du Pool pour obtenir une instance mutable.
            let mut pool_clone = (**pool_arc).clone();

            // 3. On modifie notre clone local.
            let (vault_a, _) = pool_clone.get_vaults();
            let is_vault_a = vault_address == &vault_a;

            let updated = match &mut pool_clone {
                Pool::RaydiumAmmV4(p) => {
                    if is_vault_a { p.reserve_a = new_balance } else { p.reserve_b = new_balance };
                    true
                },
                Pool::RaydiumCpmm(p) => {
                    if is_vault_a { p.reserve_a = new_balance } else { p.reserve_b = new_balance };
                    true
                },
                Pool::PumpAmm(p) => {
                    if is_vault_a { p.reserve_a = new_balance } else { p.reserve_b = new_balance };
                    true
                },
                _ => false // Ne rien faire pour les autres types de pools pour l'instant
            };

            // 4. Si une mise à jour a eu lieu, on remplace l'ancien Arc par un nouveau dans le graphe.
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
) -> Result<()> {
    // Phase 0 : Charger la LUT (inchangé)
    let lookup_table_account_data = rpc_client.get_account_data(&ADDRESS_LOOKUP_TABLE_ADDRESS).await?;
    let lookup_table_ref = AddressLookupTable::deserialize(&lookup_table_account_data)?;
    let owned_lookup_table = SdkAddressLookupTableAccount {
        key: ADDRESS_LOOKUP_TABLE_ADDRESS,
        addresses: lookup_table_ref.addresses.to_vec(),
    };

    // --- NOUVEAU FLUX OPTIMISÉ ---

    // Phase 1 : Calcul des protections basé sur le profit THÉORIQUE de l'optimiseur
    let protections = {
        let pool_sell_to = {
            let graph_guard = graph.lock().await;
            // 1. .get() retourne un &Arc<Pool>
            // 2. Le premier `*` déréférence en Arc<Pool>
            // 3. Le deuxième `*` déréférence en Pool
            // 4. .clone() copie la donnée Pool elle-même.
            (**graph_guard.pools.get(&opportunity.pool_sell_to_key).unwrap()).clone()
        };
        // On utilise le profit de l'opportunité, qui est le résultat de l'optimisation locale
        match protections::calculate_slippage_protections(
            opportunity.amount_in,
            opportunity.profit_in_lamports, // <-- Utilise le profit théorique
            pool_sell_to,
            &opportunity.token_intermediate_mint,
        ) {
            Ok(p) => p,
            Err(e) => {
                println!("[Phase 1 ERREUR] Échec calcul protections : {}", e);
                return Ok(());
            }
        }
    };

    // Phase 2 : Construction de la transaction candidate UNIQUE et FINALE
    let (final_arbitrage_tx, _final_execute_route_ix) = match transaction_builder::build_arbitrage_transaction(
        &opportunity,
        graph.clone(),
        &rpc_client,
        &payer,
        &owned_lookup_table,
        Some(&protections), // On passe directement les protections
    ).await {
        Ok(res) => res,
        Err(e) => {
            println!("[Phase 2 ERREUR] Échec construction tx finale : {}", e);
            return Ok(());
        }
    };

    // Phase 3 : La simulation parallèle UNIQUE (Validation + Découverte)
    let accounts_for_fees = vec![opportunity.pool_buy_from_key, opportunity.pool_sell_to_key];
    let sim_data = match simulator::run_simulations(rpc_client.clone(), &final_arbitrage_tx, accounts_for_fees).await {
        Ok(data) => data,
        Err(e) => {
            // Si cette simulation échoue, cela signifie que le marché a bougé et que
            // notre transaction protégée n'est plus viable. C'est notre validation.
            println!("[Phase 3 VALIDATION ÉCHOUÉE] La transaction n'est plus viable : {}", e);
            return Ok(());
        }
    };
    println!("  -> Validation et Découverte RÉUSSIES !");

    // On a maintenant les données finales et fiables
    let profit_brut_reel = sim_data.profit_brut_reel;
    let compute_units = sim_data.compute_units;
    let priority_fees = sim_data.priority_fees;

    // Phase 4 : Calculs finaux et décision d'envoi
    println!("\n--- [Phase 4] Calculs finaux et décision d'envoi ---");
    let mut recent_fees: Vec<u64> = priority_fees.iter().map(|f| f.prioritization_fee).collect();
    recent_fees.sort_unstable();
    let percentile_index = (recent_fees.len() as f64 * 0.8).floor() as usize;
    let dynamic_priority_fee_price_per_cu = *recent_fees.get(percentile_index).unwrap_or(&1000);

    let total_frais_normaux = 5000 + (compute_units * dynamic_priority_fee_price_per_cu) / 1_000_000;
    let profit_net_final = profit_brut_reel.saturating_sub(total_frais_normaux);

    println!("     -> Profit Net FINAL Calculé : {} lamports", profit_net_final);

    // Phase 5 : Envoi (Mode Simulation)
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
    println!("--- Lancement de l'Execution Bot (Version de Développement) ---");

    let config = Config::load()?;
    let payer = Keypair::from_base58_string(&config.payer_private_key);
    let rpc_client = Arc::new(ResilientRpcClient::new(config.solana_rpc_url.clone(), 3, 500));
    let main_graph = Arc::new(load_main_graph_from_cache()?);
    let hot_graph = Arc::new(Mutex::new(Graph::new()));
    let wss_url = config.solana_rpc_url.replace("http", "ws");
    let pubsub_client = Arc::new(PubsubClient::new(&wss_url).await?);
    let (update_sender, mut update_receiver) = mpsc::channel::<(Pubkey, u64)>(100);

    let _hot_graph_clone_for_task1 = Arc::clone(&hot_graph);
    let _main_graph_clone_for_task1 = Arc::clone(&main_graph);
    let _pubsub_client_clone_for_task1 = Arc::clone(&pubsub_client);
    let _update_sender_clone_for_task1 = update_sender.clone();
    let currently_processing = Arc::new(Mutex::new(HashSet::<String>::new()));

    tokio::spawn(async move {
        // ... (la logique de surveillance de la hotlist reste inchangée)
    });

    println!("[Bot] Prêt. En attente d'opportunités d'arbitrage...");
    loop {
        if let Some((vault_address, new_balance)) = update_receiver.recv().await {
            update_hot_graph_reserve(&hot_graph, &vault_address, new_balance);

            let graph_clone = Arc::clone(&hot_graph);
            let client_clone = Arc::clone(&rpc_client);
            // CORRECTION: Utilisation de la méthode moderne et gestion de l'erreur
            let payer_clone_task = Keypair::try_from(payer.to_bytes().as_slice())?;
            let processing_clone = Arc::clone(&currently_processing);

            tokio::spawn(async move {
                let opportunities = find_spatial_arbitrage(graph_clone.clone(), client_clone.clone()).await;

                if let Some(opp) = opportunities.into_iter().next() {
                    let mut pools = [opp.pool_buy_from_key.to_string(), opp.pool_sell_to_key.to_string()];
                    pools.sort();
                    let opportunity_id = format!("{}-{}", pools[0], pools[1]);

                    let is_already_processing = {
                        let mut processing_guard = processing_clone.lock().await;
                        if processing_guard.contains(&opportunity_id) { true } else { processing_guard.insert(opportunity_id.clone()); false }
                    };

                    if !is_already_processing {
                        println!("\n>>> 1 opportunité(s) détectée(s) ! Lancement du traitement...");
                        if let Err(e) = process_opportunity(opp, graph_clone, client_clone, payer_clone_task).await {
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