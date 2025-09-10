// DANS : src/bin/dev_runner.rs
use anyhow::Result;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;
use std::env;
use mev::config::Config;
use mev::rpc::ResilientRpcClient;
use mev::decoders::{
    meteora::{damm_v1, damm_v2, dlmm},
    orca::whirlpool,
    pump::amm,
    raydium::{amm_v4, clmm, cpmm},
};
use mev::state::global_cache::get_cached_clock;

// --- 2. REMPLACEZ L'ANCIENNE FONCTION get_timestamp ---
async fn get_timestamp(rpc_client: &ResilientRpcClient) -> Result<i64> {
    // La fonction devient une simple passe-plat vers notre logique de cache.
    let clock = get_cached_clock(rpc_client).await?;
    Ok(clock.unix_timestamp)
}

#[tokio::main]
async fn main() -> Result<()> {
    println!("--- Lancement du Banc d'Essai des Décodeurs ---");
    let config = Config::load()?;
    let rpc_client = ResilientRpcClient::new(config.solana_rpc_url, 3, 500);

    let payer_keypair = Keypair::from_base58_string(&config.payer_private_key);
    println!("-> Utilisation du portefeuille payeur : {}", payer_keypair.pubkey());

    // Cet appel va maintenant utiliser le cache !
    let current_timestamp = get_timestamp(&rpc_client).await?;
    println!("-> Timestamp du cluster utilisé pour tous les tests: {}", current_timestamp);

    let args: Vec<String> = env::args().skip(1).collect();

    if args.is_empty() || args.contains(&"all".to_string()) {
        println!("Mode: Exécution de tous les tests.");
        // Les appels aux fonctions de test ne changent pas, mais les fonctions elles-mêmes devront être mises à jour.
        if let Err(e) = amm_v4::test::test_ammv4_with_simulation(&rpc_client, &payer_keypair, current_timestamp).await { println!("!! AMMv4 a échoué: {}", e); }
        if let Err(e) = cpmm::test::test_cpmm_with_simulation(&rpc_client, &payer_keypair, current_timestamp).await { println!("!! CPMM a échoué: {}", e); }
        if let Err(e) = clmm::test::test_clmm(&rpc_client, &payer_keypair, current_timestamp).await { println!("!! CLMM a échoué: {}", e); }
        if let Err(e) = damm_v1::test::test_damm_v1_with_simulation(&rpc_client, &payer_keypair, current_timestamp).await { println!("!! Meteora DAMM v1 a échoué: {}", e); }
        if let Err(e) = damm_v2::test::test_damm_v2_with_simulation(&rpc_client, &payer_keypair, current_timestamp).await { println!("!! Meteora DAMM v2 a échoué: {}", e); }
        if let Err(e) = dlmm::test::test_dlmm_with_simulation(&rpc_client, &payer_keypair, current_timestamp).await { println!("!! DLMM a échoué: {}", e); }
        if let Err(e) = whirlpool::test::test_whirlpool_with_simulation(&rpc_client, &payer_keypair, current_timestamp).await { println!("!! Whirlpool a échoué: {}", e); }
        if let Err(e) = amm::test::test_amm_with_simulation(&rpc_client, &payer_keypair, current_timestamp).await { println!("!! pump.fun AMM a échoué: {}", e); }

    } else {
        println!("Mode: Exécution des tests spécifiques: {:?}", args);
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
                    println!("!! Test inconnu: '{}'", test_name);
                    continue;
                }
            };
            if let Err(e) = result {
                println!("!! Le test '{}' a échoué: {}", test_name, e);
            }
        }
    }

    println!("\n--- Banc d'essai terminé ---");
    Ok(())
}