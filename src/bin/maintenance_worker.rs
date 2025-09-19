#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

use anyhow::bail;
use std::time::Duration;
use mev::filtering::census;
use anyhow::{anyhow, Result};
use clap::{Parser, Subcommand};
use mev::{
    config::Config,
    decoders::{Pool, PoolFactory, PoolOperations},
    execution::cu_manager::DexCuCosts,
    monitoring::logging,
    rpc::ResilientRpcClient,
};
use std::time::{Instant};
use reqwest;
use mev::execution::routing::JITO_RPC_ENDPOINTS;
use solana_sdk::{
    message::VersionedMessage, pubkey::Pubkey, signature::Keypair, signer::Signer,
    transaction::VersionedTransaction,
};
use solana_client::rpc_config::RpcSimulateTransactionConfig;
use solana_transaction_status::UiTransactionEncoding;
use spl_associated_token_account::get_associated_token_address_with_program_id;
use std::{collections::HashMap, fs, str::FromStr};
use serde::{Deserialize, Serialize};
use tracing::{error, info, warn};
use std::collections::HashSet;


const RUNTIME_CONFIG_FILE: &str = "runtime_config.json";
const CU_CHANGE_ALERT_THRESHOLD_PERCENT: f64 = 15.0;

// --- Structures pour notre fichier de configuration dynamique ---
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
struct RuntimeConfig {
    compute_unit_costs: HashMap<String, DexCuCosts>,
    #[serde(default)] // Permet de ne pas avoir de crash si le champ manque
    jito_latencies: HashMap<String, u32>,
}

impl RuntimeConfig {
    fn load() -> Result<Self> {
        if let Ok(data) = fs::read_to_string(RUNTIME_CONFIG_FILE) {
            serde_json::from_str(&data).map_err(Into::into)
        } else {
            info!("[Worker/Config] Fichier {} non trouvé, création d'une configuration par défaut.", RUNTIME_CONFIG_FILE);
            Ok(RuntimeConfig::default())
        }
    }

    fn save(&self) -> Result<()> {
        let data = serde_json::to_string_pretty(self)?;
        fs::write(RUNTIME_CONFIG_FILE, data).map_err(Into::into)
    }
}

// (La structure Cli et l'enum Commands restent les mêmes)
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    Census,
    HealthCheck,
    UpdateRuntimeConfig,
    MergeHotlists,
}

#[tokio::main]
async fn main() -> Result<()> {
    logging::setup_logging();
    let cli = Cli::parse();
    let config = Config::load()?;
    let rpc_client = ResilientRpcClient::new(config.solana_rpc_url.clone(), 3, 500);

    info!("[Worker] Lancement de la tâche de maintenance...");

    // --- AJOUTEZ LE NOUVEAU MATCH ARM ---
    let task_result = match &cli.command {
        Commands::Census => run_census_task(&rpc_client).await,
        Commands::HealthCheck => run_health_check_task(&config, &rpc_client).await,
        Commands::UpdateRuntimeConfig => run_update_config_task(&config, &rpc_client).await,
        Commands::MergeHotlists => run_merge_hotlists_task().await,
    };

    if let Err(e) = task_result {
        error!("[Worker] La tâche a échoué : {:?}", e);
        std::process::exit(1);
    }

    info!("[Worker] Tâche terminée avec succès.");
    Ok(())
}

// --- AJOUTEZ CETTE NOUVELLE FONCTION COMPLÈTE EN BAS DU FICHIER ---

// On définit les noms de fichiers comme constantes pour éviter les erreurs de frappe.
const HOTLIST_FILE_NAME: &str = "hotlist.json";
const MANUAL_HOTLIST_FILE_NAME: &str = "manual_hotlist.json";
const SCANNER_HOTLIST_FILE_NAME: &str = "scanner_hotlist.json";
const COPYTRADE_HOTLIST_FILE_NAME: &str = "copytrade_hotlist.json";

/// Tâche: Fusionne toutes les sources de hotlist en un seul fichier final.
async fn run_merge_hotlists_task() -> Result<()> {
    info!("[Worker/Merge] Démarrage de la fusion des hotlists...");

    let mut final_hotlist: HashSet<Pubkey> = HashSet::new();

    // Fonction helper pour charger et fusionner une source de manière propre.
    let mut merge_source = |file_path: &str| -> Result<()> {
        if let Ok(data) = fs::read_to_string(file_path) {
            match serde_json::from_str::<HashSet<Pubkey>>(&data) {
                Ok(pools) => {
                    info!("[Worker/Merge] -> Fusion de {} pools depuis {}", pools.len(), file_path);
                    final_hotlist.extend(pools);
                },
                Err(e) => warn!("[Worker/Merge] Fichier {} corrompu, ignoré. Erreur: {}", file_path, e),
            }
        }
        // Si le fichier n'existe pas, on ne fait rien, ce qui est normal.
        Ok(())
    };

    // On fusionne toutes les sources.
    merge_source(MANUAL_HOTLIST_FILE_NAME)?;
    merge_source(SCANNER_HOTLIST_FILE_NAME)?;
    merge_source(COPYTRADE_HOTLIST_FILE_NAME)?;

    info!("[Worker/Merge] Fusion terminée. Total de {} pools uniques dans la hotlist finale.", final_hotlist.len());

    // On écrit le fichier final qui sera lu par l'arbitrage_engine.
    fs::write(HOTLIST_FILE_NAME, serde_json::to_string_pretty(&final_hotlist)?)?;

    Ok(())
}


async fn ping_jito_endpoints() -> Result<HashMap<String, u32>> {
    info!("[Worker/Config] Démarrage du ping des endpoints Jito...");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?;

    let mut latencies = HashMap::new();

    for (region, url) in JITO_RPC_ENDPOINTS.iter() {
        let start = Instant::now();
        // On fait une simple requête HEAD qui est très légère
        match client.head(*url).send().await {
            Ok(response) if response.status().is_success() => {
                let latency_ms = start.elapsed().as_millis() as u32;
                // On stocke la clé sous forme de String, comme dans la struct
                latencies.insert(format!("{:?}", region), latency_ms);
                info!("[Worker/Config] Ping Jito {:?}: {}ms", region, latency_ms);
            }
            Ok(response) => {
                warn!("[Worker/Config] Ping Jito {:?} a échoué avec le statut: {}", region, response.status());
            }
            Err(e) => {
                warn!("[Worker/Config] Ping Jito {:?} a échoué: {}", region, e);
            }
        }
    }
    info!("[Worker/Config] Ping des endpoints Jito terminé.");
    Ok(latencies)
}

/// Tâche: Exécute le recensement complet des pools.
async fn run_census_task(rpc_client: &ResilientRpcClient) -> Result<()> {
    info!("[Worker/Census] Démarrage du recensement complet...");
    census::run_census(rpc_client).await?;
    info!("[Worker/Census] Recensement terminé.");
    Ok(())
}

/// Tâche: Vérifie la santé de tous les décodeurs.
async fn run_health_check_task(config: &Config, rpc_client: &ResilientRpcClient) -> Result<()> {
    info!("[Worker/Health] Démarrage de la vérification de santé des décodeurs...");
    // Ici, nous allons réutiliser la logique de votre `dev_runner`.
    // Pour simplifier, nous créons une nouvelle fonction qui encapsule cette logique.
    // Cela évite d'avoir à gérer les arguments en ligne de commande du dev_runner.
    run_all_decoder_tests(config, rpc_client).await?;
    info!("[Worker/Health] Tous les décodeurs sont opérationnels.");
    Ok(())
}

/// Tâche: Met à jour les coûts CU et les latences.
/// Tâche: Met à jour les coûts CU et les latences.
async fn run_update_config_task(config: &Config, rpc_client: &ResilientRpcClient) -> Result<()> {
    info!("[Worker/Config] Démarrage de la mise à jour de la configuration d'exécution...");

    let mut runtime_config = RuntimeConfig::load()?;
    let old_cu_costs = runtime_config.compute_unit_costs.clone();

    // <--- AJOUT : On lance les deux analyses en parallèle pour gagner du temps ---
    let (cu_results, latency_results) = tokio::join!(
        analyze_all_dex_cus(config, rpc_client),
        ping_jito_endpoints()
    );

    // --- Traitement des résultats de l'analyseur de CU ---
    match cu_results {
        Ok(new_cu_costs) => {
            for (dex_name, new_cost) in &new_cu_costs {
                if let Some(old_cost) = old_cu_costs.get(dex_name.as_str()) {
                    if old_cost.base_cost > 0 {
                        let change_percent = ((new_cost.base_cost as f64 - old_cost.base_cost as f64) / old_cost.base_cost as f64).abs() * 100.0;
                        if change_percent > CU_CHANGE_ALERT_THRESHOLD_PERCENT {
                            warn!(
                                dex = %dex_name,
                                old_cost = old_cost.base_cost,
                                new_cost = new_cost.base_cost,
                                "Changement significatif de CU détecté (+{:.2}%)!", change_percent
                            );
                        }
                    }
                }
            }
            runtime_config.compute_unit_costs = new_cu_costs;
        }
        Err(e) => {
            error!("[Worker/Config] L'analyse des CUs a échoué et ne sera pas mise à jour : {}", e);
        }
    }

    // <--- AJOUT : Traitement des résultats du pinger Jito ---
    match latency_results {
        Ok(new_latencies) => {
            runtime_config.jito_latencies = new_latencies;
        }
        Err(e) => {
            error!("[Worker/Config] Le ping des endpoints Jito a échoué et ne sera pas mis à jour : {}", e);
        }
    }

    // Sauvegarder la configuration, même si une des tâches a échoué.
    // L'autre aura pu mettre à jour ses données.
    runtime_config.save()?;
    info!("[Worker/Config] Fichier {} mis à jour avec succès.", RUNTIME_CONFIG_FILE);
    Ok(())
}


/// Exécute une analyse de CU pour tous les principaux types de DEX.
async fn analyze_all_dex_cus(config: &Config, rpc_client: &ResilientRpcClient) -> Result<HashMap<String, DexCuCosts>> {
    let payer = Keypair::from_base58_string(&config.payer_private_key);
    let pool_factory = PoolFactory::new(std::sync::Arc::new(rpc_client.clone()));

    let wsol_mint = "So11111111111111111111111111111111111111112";
    let pools_to_test = vec![
        ("Raydium AMM V4", "58oQChx4yWmvKdwLLZzBi4ChoCc2fqCUWBkwMihLYQo2", wsol_mint, 0.01),
        ("Raydium CPMM", "8ujpQXxnnWvRohU2oCe3eaSzoL7paU2uj3fEn4Zp72US", wsol_mint, 0.01),
        ("Raydium CLMM", "YrrUStgPugDp8BbfosqDeFssen6sA75ZS1QJvgnHtmY", wsol_mint, 0.01),
        ("Orca Whirlpool", "Czfq3xZZDmsdGdUyrNLtRhGc47cXcZtLG4crryfu44zE", wsol_mint, 0.01),
        ("Meteora DAMM V1", "5rCf1DM8LjKTw4YqhnoLcngyZYeNnQqztScTogYHAS6", wsol_mint, 0.01),
        ("Meteora DLMM", "GcnHKJgMxeUCy7PUcVEssZ6swiAUt9KFPky3EjSLJL3f", wsol_mint, 0.01),
        ("Meteora DAMM V2", "FiMTgvjJq7dWX5ZetXZv6XeHxXSYJRG2SgNR9mygs9KN", wsol_mint, 0.01),
        ("Pump AMM", "CLYFHhJfJjNPSMQv7byFeAsZ8x1EXQyYkGTPrNc2vc78", wsol_mint, 0.01),
    ];

    let mut results_map = HashMap::new();
    for (name, pool_address, input_mint, amount_in) in pools_to_test {
        info!("[Worker/CU] Analyse de {}...", name);
        match analyze_pool(rpc_client, &pool_factory, &payer, pool_address, input_mint, amount_in).await {
            Ok(units) => {
                results_map.insert(name.to_string(), DexCuCosts { base_cost: units, cost_per_tick: None });
            }
            Err(e) => {
                error!("[Worker/CU] Échec de l'analyse pour {}: {}", name, e);
            }
        }
    }
    // Note: La détection du `cost_per_tick` reste un processus manuel pour l'instant.
    // On pourrait l'automatiser en lançant un deuxième test avec un montant plus élevé.
    Ok(results_map)
}

async fn analyze_pool(
    rpc_client: &ResilientRpcClient,
    pool_factory: &PoolFactory,
    payer: &Keypair,
    pool_address: &str,
    input_mint_address: &str,
    amount_in_ui: f64,
) -> Result<u64> {
    let pool_pubkey = Pubkey::from_str(pool_address)?;
    let input_mint_pubkey = Pubkey::from_str(input_mint_address)?;
    let hydrated_pool = pool_factory.create_and_hydrate_pool(&pool_pubkey).await?;
    let (mint_a, _mint_b) = hydrated_pool.get_mints();
    let decimals = if input_mint_pubkey == mint_a {
        get_mint_a_decimals(&hydrated_pool)
    } else {
        get_mint_b_decimals(&hydrated_pool)
    };
    let amount_in_base = (amount_in_ui * 10f64.powi(decimals as i32)) as u64;
    execute_and_get_cus(rpc_client, payer, hydrated_pool, input_mint_pubkey, amount_in_base).await
}

async fn execute_and_get_cus(
    rpc_client: &ResilientRpcClient,
    payer: &Keypair,
    pool: Pool,
    token_in_mint: Pubkey,
    amount_in: u64,
) -> Result<u64> {

    let (mint_a, mint_b) = pool.get_mints();
    let token_out_mint = if token_in_mint == mint_a { mint_b } else { mint_a };
    let (input_token_program, output_token_program) = match &pool {
        Pool::RaydiumClmm(p) if token_in_mint == p.mint_a => (p.mint_a_program, p.mint_b_program),
        Pool::RaydiumClmm(p) => (p.mint_b_program, p.mint_a_program),
        Pool::OrcaWhirlpool(p) if token_in_mint == p.mint_a => (p.mint_a_program, p.mint_b_program),
        Pool::OrcaWhirlpool(p) => (p.mint_b_program, p.mint_a_program),
        Pool::MeteoraDlmm(p) if token_in_mint == p.mint_a => (p.mint_a_program, p.mint_b_program),
        Pool::MeteoraDlmm(p) => (p.mint_b_program, p.mint_a_program),
        Pool::MeteoraDammV2(p) if token_in_mint == p.mint_a => (p.mint_a_program, p.mint_b_program),
        Pool::MeteoraDammV2(p) => (p.mint_b_program, p.mint_a_program),
        Pool::RaydiumCpmm(p) if token_in_mint == p.token_0_mint => (p.token_0_program, p.token_1_program),
        Pool::RaydiumCpmm(p) => (p.token_1_program, p.token_0_program),
        Pool::PumpAmm(p) if token_in_mint == p.mint_a => (p.mint_a_program, p.mint_b_program),
        Pool::PumpAmm(p) => (p.mint_b_program, p.mint_a_program),
        _ => (spl_token::ID, spl_token::ID),
    };
    let user_accounts = mev::decoders::pool_operations::UserSwapAccounts {
        owner: payer.pubkey(),
        source: get_associated_token_address_with_program_id(&payer.pubkey(), &token_in_mint, &input_token_program),
        destination: get_associated_token_address_with_program_id(&payer.pubkey(), &token_out_mint, &output_token_program),
    };
    let swap_ix = pool.create_swap_instruction(&token_in_mint, amount_in, 1, &user_accounts)?;
    let mut instructions = Vec::new();
    if rpc_client.get_account(&user_accounts.destination).await.is_err() {
        instructions.push(
            spl_associated_token_account::instruction::create_associated_token_account(
                &payer.pubkey(), &payer.pubkey(), &token_out_mint, &output_token_program,
            ),
        );
    }
    instructions.push(swap_ix);
    let recent_blockhash = rpc_client.get_latest_blockhash().await?;
    let message = VersionedMessage::V0(solana_sdk::message::v0::Message::try_compile(&payer.pubkey(), &instructions, &[], recent_blockhash)?);
    let transaction = VersionedTransaction::try_new(message, &[payer])?;
    let sim_config = RpcSimulateTransactionConfig {
        sig_verify: false, replace_recent_blockhash: true, commitment: Some(rpc_client.commitment()),
        encoding: Some(UiTransactionEncoding::Base64), accounts: None, min_context_slot: None,
        inner_instructions: false,
    };
    let sim_result = rpc_client.simulate_transaction_with_config(&transaction, sim_config).await?.value;
    if let Some(err) = sim_result.err {
        return Err(anyhow!("La simulation a échoué: {:?}, logs: {:?}", err, sim_result.logs));
    }
    sim_result.units_consumed.ok_or_else(|| anyhow!("Units consumed non disponible."))
}

fn get_mint_a_decimals(pool: &Pool) -> u8 {
    match pool {
        Pool::RaydiumAmmV4(p) => p.mint_a_decimals,
        Pool::RaydiumCpmm(p) => p.mint_0_decimals,
        Pool::RaydiumClmm(p) => p.mint_a_decimals,
        Pool::MeteoraDammV1(p) => p.mint_a_decimals,
        Pool::MeteoraDammV2(p) => p.mint_a_decimals,
        Pool::MeteoraDlmm(p) => p.mint_a_decimals,
        Pool::OrcaWhirlpool(p) => p.mint_a_decimals,
        Pool::PumpAmm(p) => p.mint_a_decoded.decimals,
    }
}

fn get_mint_b_decimals(pool: &Pool) -> u8 {
    match pool {
        Pool::RaydiumAmmV4(p) => p.mint_b_decimals,
        Pool::RaydiumCpmm(p) => p.mint_1_decimals,
        Pool::RaydiumClmm(p) => p.mint_b_decimals,
        Pool::MeteoraDammV1(p) => p.mint_b_decimals,
        Pool::MeteoraDammV2(p) => p.mint_b_decimals,
        Pool::MeteoraDlmm(p) => p.mint_b_decimals,
        Pool::OrcaWhirlpool(p) => p.mint_b_decimals,
        Pool::PumpAmm(p) => p.mint_b_decoded.decimals,
    }
}

/// Helper qui exécute tous les tests de décodeurs (logique de `dev_runner`).

async fn run_all_decoder_tests(config: &Config, rpc_client: &ResilientRpcClient) -> Result<()> {
    use mev::decoders::{meteora, orca, pump, raydium};
    use anyhow::bail; // Assurez-vous que `bail` est importé

    let payer = config.payer_keypair()?; // Le '?' est correct ici car payer_keypair retourne un Result
    let clock = mev::state::global_cache::get_cached_clock(rpc_client).await?;
    let ts = clock.unix_timestamp;

    let mut all_ok = true;

    info!("Lancement du test Raydium AMM V4...");
    if let Err(e) = raydium::amm_v4::test::test_ammv4_with_simulation(rpc_client, &payer, ts).await {
        error!("[Health] ÉCHEC Raydium AMM V4: {:?}", e); all_ok = false;
    }
    info!("Lancement du test Raydium CPMM...");
    if let Err(e) = raydium::cpmm::test::test_cpmm_with_simulation(rpc_client, &payer, ts).await {
        error!("[Health] ÉCHEC Raydium CPMM: {:?}", e); all_ok = false;
    }
    info!("Lancement du test Raydium CLMM...");
    if let Err(e) = raydium::clmm::test::test_clmm(rpc_client, &payer, ts).await {
        error!("[Health] ÉCHEC Raydium CLMM: {:?}", e); all_ok = false;
    }
    info!("Lancement du test Orca Whirlpool...");
    if let Err(e) = orca::whirlpool::test::test_whirlpool_with_simulation(rpc_client, &payer, ts).await {
        error!("[Health] ÉCHEC Orca Whirlpool: {:?}", e); all_ok = false;
    }
    info!("Lancement du test Meteora DAMM V1...");
    if let Err(e) = meteora::damm_v1::test::test_damm_v1_with_simulation(rpc_client, &payer, ts).await {
        error!("[Health] ÉCHEC Meteora DAMM V1: {:?}", e); all_ok = false;
    }
    info!("Lancement du test Meteora DAMM V2...");
    if let Err(e) = meteora::damm_v2::test::test_damm_v2_with_simulation(rpc_client, &payer, ts).await {
        error!("[Health] ÉCHEC Meteora DAMM V2: {:?}", e); all_ok = false;
    }
    info!("Lancement du test Meteora DLMM...");
    if let Err(e) = meteora::dlmm::test::test_dlmm_with_simulation(rpc_client, &payer, ts).await {
        error!("[Health] ÉCHEC Meteora DLMM: {:?}", e); all_ok = false;
    }
    info!("Lancement du test Pump AMM...");
    if let Err(e) = pump::amm::test::test_amm_with_simulation(rpc_client, &payer, ts).await {
        error!("[Health] ÉCHEC Pump AMM: {:?}", e); all_ok = false;
    }

    if !all_ok {
        bail!("Un ou plusieurs tests de décodeur ont échoué.");
    }
    Ok(())
}

