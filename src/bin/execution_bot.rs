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
use anchor_lang::{AnchorSerialize}; // Pour sérialiser nos instructions pour Anchor
use solana_sdk::instruction::AccountMeta; // Pour éviter l'ambiguïté avec d'autres types
use mev::decoders::pool_operations::UserSwapAccounts; // La struct pour les comptes utilisateur
use spl_associated_token_account::get_associated_token_address;
use anchor_lang::prelude::*;
use anchor_lang::InstructionData;
use solana_sdk::transaction::VersionedTransaction;
use solana_sdk::message::{v0, VersionedMessage};
use solana_address_lookup_table_program::state::AddressLookupTable;
use std::env::args;

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


async fn build_arbitrage_transaction(
    opportunity: &ArbitrageOpportunity,
    graph: Arc<Mutex<Graph>>,
    rpc_client: &RpcClient,
    payer: &Keypair,
    lookup_table_account: &AddressLookupTable<'_>,
) -> Result<VersionedTransaction> {
    println!("\n--- [Phase 1] Construction de la Transaction V0 (avec LUT) ---");

    // --- A. Préparation des instructions de swap (inchangé) ---
    let graph_guard = graph.lock().await;
    let mut pool_buy_from = graph_guard.pools.get(&opportunity.pool_buy_from_key).unwrap().clone();
    let mut pool_sell_to = graph_guard.pools.get(&opportunity.pool_sell_to_key).unwrap().clone();
    drop(graph_guard);

    let predicted_intermediate_out = pool_buy_from.get_quote_async(
        &opportunity.token_in_mint,
        opportunity.amount_in,
        rpc_client,
    ).await?;

    let user_accounts_buy = UserSwapAccounts {
        owner: payer.pubkey(),
        source: get_associated_token_address(&payer.pubkey(), &opportunity.token_in_mint),
        destination: get_associated_token_address(&payer.pubkey(), &opportunity.token_intermediate_mint),
    };
    let user_accounts_sell = UserSwapAccounts {
        owner: payer.pubkey(),
        source: get_associated_token_address(&payer.pubkey(), &opportunity.token_intermediate_mint),
        destination: get_associated_token_address(&payer.pubkey(), &opportunity.token_in_mint),
    };

    let ix_buy = pool_buy_from.create_swap_instruction(&opportunity.token_in_mint, opportunity.amount_in, 1, &user_accounts_buy)?;
    let ix_sell = pool_sell_to.create_swap_instruction(&opportunity.token_intermediate_mint, predicted_intermediate_out, 1, &user_accounts_sell)?;

    // --- B. Préparation des données pour notre programme (inchangé) ---
    let step1 = ProgramSwapStep {
        dex_program_id: ix_buy.program_id,
        num_accounts_for_step: ix_buy.accounts.len() as u8,
        instruction_data: ix_buy.data,
    };
    let step2 = ProgramSwapStep {
        dex_program_id: ix_sell.program_id,
        num_accounts_for_step: ix_sell.accounts.len() as u8,
        instruction_data: ix_sell.data,
    };
    let mut args = ExecuteRouteIxArgs { route: vec![step1, step2], minimum_expected_profit: 0 }; // <--- AJOUTEZ 'mut'
    let mut instruction_data = Vec::new();
    // Le discriminateur correct pour "global::execute_route"
    instruction_data.extend_from_slice(&[246, 14, 81, 121, 140, 237, 86, 23]);
    instruction_data.extend_from_slice(&args.try_to_vec()?);


    // --- C. Construction de l'instruction COMPLÈTE (Logique corrigée) ---
    let (config_pda, _) = Pubkey::find_program_address(&[b"config"], &ATOMIC_ARB_EXECUTOR_PROGRAM_ID);

    // D'abord, les comptes attendus par la structure `ExecuteRoute`
    let mut final_accounts_list = vec![
        AccountMeta::new_readonly(config_pda, false), // `config`
        AccountMeta::new(payer.pubkey(), true),      // `signer`
    ];

    // Ensuite, on ajoute les `remaining_accounts`
    // 1. Le compte de profit
    final_accounts_list.push(AccountMeta { pubkey: user_accounts_sell.destination, is_signer: false, is_writable: true });

    // 2. Les comptes du premier swap
    //    *** LA CORRECTION EST ICI : On ne retire PLUS le statut de signataire ***
    final_accounts_list.extend(ix_buy.accounts);

    // 3. Les comptes du deuxième swap
    //    *** LA CORRECTION EST ICI : On ne retire PLUS le statut de signataire ***
    final_accounts_list.extend(ix_sell.accounts);

    let execute_route_ix = Instruction {
        program_id: ATOMIC_ARB_EXECUTOR_PROGRAM_ID,
        accounts: final_accounts_list.clone(), // On utilise notre liste corrigée
        data: instruction_data,
    };

    println!("\n--- COMPTES REQUIS PAR LE BOT RUST ---");
    for (i, acc) in final_accounts_list.iter().enumerate() {
        println!("{:2}: {} (Signer: {})", i, acc.pubkey, acc.is_signer);
    }
    println!("-------------------------------------\n");

    // --- D. Construction de la VersionedTransaction (inchangé) ---
    let recent_blockhash = rpc_client.get_latest_blockhash().await?;

    let sdk_lookup_table = solana_sdk::message::AddressLookupTableAccount {
        key: ADDRESS_LOOKUP_TABLE_ADDRESS,
        addresses: lookup_table_account.addresses.to_vec(),
    };

    let message = v0::Message::try_compile(
        &payer.pubkey(),
        &[execute_route_ix],
        &[sdk_lookup_table],
        recent_blockhash,
    )?;

    let transaction = VersionedTransaction::try_new(VersionedMessage::V0(message), &[payer])?;

    println!("  -> Transaction V0 construite avec succès.");
    Ok(transaction)
}


// ID de votre programme on-chain, récupéré depuis Anchor.toml
const ATOMIC_ARB_EXECUTOR_PROGRAM_ID: Pubkey = pubkey!("3gHUHkQD8TjeQntEsygDnm4TRo3xKQRTbDTaFxgQdXe1");

const ADDRESS_LOOKUP_TABLE_ADDRESS: Pubkey = solana_sdk::pubkey!("E5h798UBdK8V1L7MvRfi1ppr2vitPUUUUCVqvTyDgKXN");

// Une réplique exacte de la struct `SwapStep` de votre programme on-chain.
#[derive(AnchorSerialize, Clone, Debug)]
pub struct ProgramSwapStep {
    pub dex_program_id: Pubkey,
    pub num_accounts_for_step: u8,
    pub instruction_data: Vec<u8>,
}

// Une réplique des arguments attendus par votre instruction `execute_route`.
#[derive(AnchorSerialize)]
pub struct ExecuteRouteIxArgs {
    pub route: Vec<ProgramSwapStep>,
    pub minimum_expected_profit: u64,
}

/// Extrait le profit réalisé à partir des logs d'une simulation réussie de `execute_route`.
fn parse_profit_from_logs(logs: &[String]) -> Option<u64> {
    let prefix = "Program log: SUCCÈS ! Profit net réalisé: ";
    for log in logs {
        if let Some(profit_str) = log.strip_prefix(prefix) {
            return profit_str.parse::<u64>().ok();
        }
    }
    None
}

/// Orchestre le cycle de vie complet du traitement d'une opportunité d'arbitrage.
async fn process_opportunity(
    opportunity: ArbitrageOpportunity,
    graph: Arc<Mutex<Graph>>,
    rpc_client: Arc<RpcClient>,
    payer: Keypair,
) -> Result<()> {
    // --- Phase 0 : Charger la LUT ---
    let lookup_table_account_data = rpc_client.get_account_data(&ADDRESS_LOOKUP_TABLE_ADDRESS).await?;
    let lookup_table_account = AddressLookupTable::deserialize(&lookup_table_account_data)?;

    // --- Phase 1 : Construction de la transaction V0 ---
    let arbitrage_tx = match build_arbitrage_transaction(
        &opportunity,
        graph.clone(),
        &rpc_client,
        &payer,
        &lookup_table_account, // On passe la LUT chargée
    ).await {
        Ok(tx) => tx,
        Err(e) => {
            println!("[Phase 1 ERREUR] Échec de la construction de la transaction V0 : {}", e);
            return Ok(());
        }
    };

    // --- Le reste de la fonction (simulations, etc.) reste identique ---
    // Elle fonctionnera maintenant car `arbitrage_tx` est une VersionedTransaction,
    // que `simulate_transaction_with_config` sait gérer.

    println!("\n--- [Phase 2] Lancement des simulations et de la récupération des frais ---");
    let sim_config = RpcSimulateTransactionConfig {
        sig_verify: false,
        replace_recent_blockhash: true,
        commitment: Some(rpc_client.commitment()),
        encoding: Some(solana_transaction_status::UiTransactionEncoding::Base64),
        ..Default::default()
    };
    let accounts_for_fees: Vec<Pubkey> = vec![
        opportunity.pool_buy_from_key,
        opportunity.pool_sell_to_key
    ];
    let (sim_result, jito_sim_result, priority_fees_result) = tokio::join!(
        rpc_client.simulate_transaction_with_config(&arbitrage_tx, sim_config.clone()),
        rpc_client.simulate_transaction_with_config(&arbitrage_tx, sim_config),
        rpc_client.get_recent_prioritization_fees(&accounts_for_fees)
    );

    // ... (le reste de votre logique de Phase 3, 4, 5, 6 est correct et n'a pas besoin de changer)

    println!("\n--- [Phase 3] Analyse des résultats ---");
    let sim_response = match jito_sim_result.or(sim_result) { Ok(response) => response, Err(e) => { println!("  -> Les deux simulations ont échoué. Erreur RPC : {}", e); return Ok(()); } };
    let sim_value = sim_response.value;
    if let Some(err) = sim_value.err {
        println!("  -> Simulation ÉCHOUÉE : {:?}", err);
        if let Some(logs) = sim_value.logs { logs.iter().for_each(|log| println!("       {}", log)); }
        return Ok(());
    }
    println!("  -> Simulation RÉUSSIE !");
    let compute_units = sim_value.units_consumed.unwrap_or(0);
    let logs = sim_value.logs.unwrap_or_default();

    let profit_brut_reel = match parse_profit_from_logs(&logs) {
        Some(profit) => profit,
        None => {
            println!("  -> ERREUR : Impossible d'extraire le profit des logs de simulation.");
            return Ok(());
        }
    };

    println!("     -> Profit Brut Réel (simulé) : {} lamports", profit_brut_reel);
    println!("     -> Compute Units Consommés   : {}", compute_units);

    // --- Phase 4 : Calcul Dynamique et Décision ---
    println!("\n--- [Phase 4] Calculs dynamiques et décision ---");

    let priority_fees = match priority_fees_result {
        Ok(fees) => fees,
        Err(e) => {
            println!("  -> AVERTISSEMENT : Impossible de récupérer les frais de priorité : {}", e);
            // On utilise une valeur par défaut sûre si l'appel RPC échoue
            vec![]
        }
    };

    // Stratégie simple : on prend le 80ème percentile des frais des 20 derniers slots.
    let mut recent_fees: Vec<u64> = priority_fees.iter().map(|f| f.prioritization_fee).collect();
    recent_fees.sort_unstable();
    let percentile_index = (recent_fees.len() as f64 * 0.8).floor() as usize;
    let dynamic_priority_fee_price_per_cu = *recent_fees.get(percentile_index).unwrap_or(&1000); // 1000 micro-lamports par CU par défaut

    let total_frais_normaux = 5000 + (compute_units * dynamic_priority_fee_price_per_cu) / 1_000_000; // 5000 base fee + priority fee
    let profit_net_avant_tip = profit_brut_reel.saturating_sub(total_frais_normaux);

    println!("     -> Prix Priorité Dynamique   : {} micro-lamports/CU", dynamic_priority_fee_price_per_cu);
    println!("     -> Coût Total Est. (Normal)  : {} lamports", total_frais_normaux);
    println!("     -> Profit Net Est. (Normal)  : {} lamports", profit_net_avant_tip);

    if profit_net_avant_tip == 0 {
        println!("  -> DÉCISION : Abandon. Opportunité non rentable après frais de priorité.");
        return Ok(());
    }

    // Calcul du Jito Tip (80% du profit net restant)
    const POURCENTAGE_PROFIT_POUR_JITO_TIP: f64 = 0.80;
    let jito_tip = (profit_net_avant_tip as f64 * POURCENTAGE_PROFIT_POUR_JITO_TIP).round() as u64;
    let profit_apres_jito = profit_net_avant_tip.saturating_sub(jito_tip);

    println!("     -> Jito Tip Calculé            : {} lamports", jito_tip);
    println!("     -> Profit Net Est. (Jito)      : {} lamports", profit_apres_jito);

    if profit_apres_jito == 0 {
        println!("  -> DÉCISION : N'enverra pas à Jito, le tip rend l'opportunité non rentable.");
    }

    // --- Phase 5 : Construction des Transactions Finales ---
    println!("\n--- [Phase 5] Construction des transactions finales ---");
    // TODO: Implémenter le calcul du slippage toléré.
    // TODO: Implémenter le calcul inverse pour le `min_amount_out` du premier swap.
    // TODO: Reconstruire l'instruction `execute_route` avec les nouvelles protections.
    // TODO: Construire la transaction normale et le bundle Jito.

    // --- Phase 6 : Envoi ---
    println!("\n--- [Phase 6] Envoi (Simulation) ---");
    if profit_net_avant_tip > 0 {
        println!("  -> [SIM] Enverrait la transaction normale.");
    }
    if profit_apres_jito > 0 {
        println!("  -> [SIM] Enverrait le bundle Jito.");
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
    let currently_processing = Arc::new(Mutex::new(HashSet::<String>::new()));
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
                    *hot_graph_clone_for_task1.lock().await = new_hot_graph;
                    current_subscribed_vaults = target_vaults;
                    println!("[Hotlist] Abonnements actifs: {}", current_subscribed_vaults.len());
                }
            }
            time::sleep(time::Duration::from_secs(5)).await;
        }
    });

    println!("[Bot] Prêt. En attente d'opportunités d'arbitrage...");

    // On prépare un clone du payeur qui pourra être déplacé dans les tâches
    let payer_cloneable = Keypair::try_from(payer.to_bytes().as_slice())?;

    println!("[Bot] Prêt. En attente d'opportunités d'arbitrage...");
    loop {
        if let Some((vault_address, new_balance)) = update_receiver.recv().await {
            update_hot_graph_reserve(&hot_graph, &vault_address, new_balance);

            let graph_clone = Arc::clone(&hot_graph);
            let client_clone = Arc::clone(&rpc_client);
            let payer_clone = Keypair::from_bytes(&payer.to_bytes())?;
            let processing_clone = Arc::clone(&currently_processing); // On clone le verrou

            tokio::spawn(async move {
                let opportunities = find_spatial_arbitrage(graph_clone.clone(), client_clone.clone()).await;

                // On ne traite que la première opportunité trouvée pour éviter de spammer
                // les logs et les simulations pour un seul événement.
                if let Some(opp) = opportunities.into_iter().next() {

                    // 1. Créer l'ID unique
                    let mut pools = [opp.pool_buy_from_key.to_string(), opp.pool_sell_to_key.to_string()];
                    pools.sort();
                    let opportunity_id = format!("{}-{}", pools[0], pools[1]);

                    // 2. Vérifier et acquérir le verrou
                    let is_already_processing = {
                        let mut processing_guard = processing_clone.lock().await;
                        if processing_guard.contains(&opportunity_id) {
                            true
                        } else {
                            processing_guard.insert(opportunity_id.clone());
                            false
                        }
                    };

                    if !is_already_processing {
                        println!("\n>>> 1 opportunité(s) détectée(s) ! Lancement du traitement...");

                        // 3. On appelle VOTRE fonction de traitement
                        if let Err(e) = process_opportunity(opp, graph_clone, client_clone, payer_clone).await {
                            println!("[Erreur Traitement] {}", e);
                        }

                        // 4. Libérer le verrou
                        let mut processing_guard = processing_clone.lock().await;
                        processing_guard.remove(&opportunity_id);
                    }
                }
            });
        }
    }
}