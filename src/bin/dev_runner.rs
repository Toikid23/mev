// DANS : src/bin/dev_runner.rs

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

use anyhow::Result;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;
use std::env;
use mev::{
    config::Config,
    decoders::{
        meteora::{damm_v1, damm_v2, dlmm},
        orca::whirlpool,
        pump::amm,
        raydium::{amm_v4, clmm, cpmm},
    },
    monitoring,
    rpc::ResilientRpcClient,
    state::global_cache::get_cached_clock,
};
// <-- MODIFIÉ : Ajout des imports tracing
use tracing::{error, info, warn};

async fn get_timestamp(rpc_client: &ResilientRpcClient) -> Result<i64> {
    let clock = get_cached_clock(rpc_client).await?;
    Ok(clock.unix_timestamp)
}

#[tokio::main]
async fn main() -> Result<()> {
    monitoring::logging::setup_logging();

    info!("--- Lancement du Banc d'Essai des Décodeurs ---");
    let config = Config::load()?;
    let rpc_client = ResilientRpcClient::new(config.solana_rpc_url, 3, 500);

    let payer_keypair = Keypair::from_base58_string(&config.payer_private_key);
    info!(payer = %payer_keypair.pubkey(), "Utilisation du portefeuille payeur");

    let current_timestamp = get_timestamp(&rpc_client).await?;
    info!(timestamp = current_timestamp, "Timestamp du cluster utilisé pour tous les tests");

    let args: Vec<String> = env::args().skip(1).collect();


    if args.is_empty() || args.contains(&"all".to_string()) {
        info!("--- Mode: Exécution de tous les tests ---");
        if let Err(e) = amm_v4::test::test_ammv4_with_simulation(&rpc_client, &payer_keypair, current_timestamp).await { error!(protocol="AMMv4", error = %e, "Test échoué"); }
        if let Err(e) = cpmm::test::test_cpmm_with_simulation(&rpc_client, &payer_keypair, current_timestamp).await { error!(protocol="CPMM", error = %e, "Test échoué"); }
        if let Err(e) = clmm::test::test_clmm(&rpc_client, &payer_keypair, current_timestamp).await { error!(protocol="CLMM", error = %e, "Test échoué"); }
        if let Err(e) = damm_v1::test::test_damm_v1_with_simulation(&rpc_client, &payer_keypair, current_timestamp).await { error!(protocol="Meteora DAMM v1", error = %e, "Test échoué"); }
        if let Err(e) = damm_v2::test::test_damm_v2_with_simulation(&rpc_client, &payer_keypair, current_timestamp).await { error!(protocol="Meteora DAMM v2", error = %e, "Test échoué"); }
        if let Err(e) = dlmm::test::test_dlmm_with_simulation(&rpc_client, &payer_keypair, current_timestamp).await { error!(protocol="Meteora DLMM", error = %e, "Test échoué"); }
        if let Err(e) = whirlpool::test::test_whirlpool_with_simulation(&rpc_client, &payer_keypair, current_timestamp).await { error!(protocol="Orca Whirlpool", error = %e, "Test échoué"); }
        if let Err(e) = amm::test::test_amm_with_simulation(&rpc_client, &payer_keypair, current_timestamp).await { error!(protocol="Pump.fun AMM", error = %e, "Test échoué"); }
    } else {
        info!(tests = ?args, "--- Mode: Exécution de tests spécifiques ---");
        for test_name in args {
            let result = match test_name.as_str() {
                "amm_v4" => amm_v4::test::test_ammv4_with_simulation(&rpc_client, &payer_keypair, current_timestamp).await,
                "cpmm" => cpmm::test::test_cpmm_with_simulation(&rpc_client, &payer_keypair, current_timestamp).await,
                "clmm" => clmm::test::test_clmm(&rpc_client, &payer_keypair, current_timestamp).await,
                "damm_v1" => damm_v1::test::test_damm_v1_with_simulation(&rpc_client, &payer_keypair, current_timestamp).await,
                "damm_v2" => damm_v2::test::test_damm_v2_with_simulation(&rpc_client, &payer_keypair, current_timestamp).await,
                "dlmm" => dlmm::test::test_dlmm_with_simulation(&rpc_client, &payer_keypair, current_timestamp).await,
                "whirlpool" => whirlpool::test::test_whirlpool_with_simulation(&rpc_client, &payer_keypair, current_timestamp).await,
                "pump_amm" => amm::test::test_amm_with_simulation(&rpc_client, &payer_keypair, current_timestamp).await,
                _ => {
                    warn!(test_name = %test_name, "Test inconnu");
                    continue;
                }
            };
            if let Err(e) = result {
                error!(test_name = %test_name, error = %e, "Test échoué");
            }
        }
    }

    info!("--- Banc d'essai terminé ---");
    Ok(())
}