use anyhow::Result;
use bincode::deserialize;
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;
use solana_sdk::sysvar::clock::Clock;
use std::env;
use mev::config::Config;
use mev::decoders::{
    meteora::{damm_v1, damm_v2, dlmm},
    orca::{amm_v1, amm_v2, whirlpool},
    pump::amm,
    raydium::{amm_v4, clmm, cpmm, launchpad, stable_swap},
};

async fn get_timestamp(rpc_client: &RpcClient) -> Result<i64> {
    let clock_account = rpc_client
        .get_account(&solana_sdk::sysvar::clock::ID)
        .await?;
    let clock: Clock = deserialize(&clock_account.data)?;
    Ok(clock.unix_timestamp)
}

#[tokio::main]
async fn main() -> Result<()> {
    println!("--- Lancement du Banc d'Essai des Décodeurs ---");
    let config = Config::load()?;
    let rpc_client = RpcClient::new(config.solana_rpc_url);

    // On charge le portefeuille depuis la clé privée dans le .env
    let payer_keypair = Keypair::from_base58_string(&config.payer_private_key);
    println!("-> Utilisation du portefeuille payeur : {}", payer_keypair.pubkey());

    let current_timestamp = get_timestamp(&rpc_client).await?;
    println!("-> Timestamp du cluster utilisé pour tous les tests: {}", current_timestamp);

    let args: Vec<String> = env::args().skip(1).collect();

    if args.is_empty() || args.contains(&"all".to_string()) {
        println!("Mode: Exécution de tous les tests.");


        if let Err(e) = amm_v4::test::test_ammv4_with_simulation(&rpc_client, &payer_keypair, current_timestamp).await { println!("!! AMMv4 a échoué: {}", e); }
        if let Err(e) = cpmm::test::test_cpmm_with_simulation(&rpc_client, &payer_keypair, current_timestamp).await { println!("!! CPMM a échoué: {}", e); }
        if let Err(e) = clmm::test::test_clmm(&rpc_client, &payer_keypair, current_timestamp).await { println!("!! CLMM a échoué: {}", e); }
        if let Err(e) = launchpad::test::test_launchpad(&rpc_client, current_timestamp).await { println!("!! Launchpad a échoué: {}", e); }
        if let Err(e) = stable_swap::test::test_stable_swap(&rpc_client, current_timestamp).await { println!("!! Stable Swap a échoué: {}", e); }
        if let Err(e) = damm_v1::test::test_damm_v1_with_simulation(&rpc_client, &payer_keypair, current_timestamp).await { println!("!! Meteora DAMM v1 a échoué: {}", e); }
        if let Err(e) = damm_v2::test::test_damm_v2_with_simulation(&rpc_client, &payer_keypair, current_timestamp).await { println!("!! Meteora DAMM v2 a échoué: {}", e); }
        if let Err(e) = dlmm::test::test_dlmm_with_simulation(&rpc_client, &payer_keypair, current_timestamp).await { println!("!! DLMM a échoué: {}", e); }
        if let Err(e) = whirlpool::test::test_whirlpool_with_simulation(&rpc_client, &payer_keypair, current_timestamp).await { println!("!! Whirlpool a échoué: {}", e); }
        if let Err(e) = amm_v2::test::test_orca_amm_v2(&rpc_client, current_timestamp).await { println!("!! Orca AMM V2 a échoué: {}", e); }
        if let Err(e) = amm_v1::test::test_orca_amm_v1(&rpc_client, current_timestamp).await { println!("!! Orca AMM V1 a échoué: {}", e); }
        if let Err(e) = amm::test::test_amm_with_simulation(&rpc_client, &payer_keypair, current_timestamp).await { println!("!! pump.fun AMM a échoué: {}", e); }

    } else {
        println!("Mode: Exécution des tests spécifiques: {:?}", args);
        for test_name in args {
            let result = match test_name.as_str() {
                "amm_v4" => amm_v4::test::test_ammv4_with_simulation(&rpc_client, &payer_keypair, current_timestamp).await,
                "cpmm" => cpmm::test::test_cpmm_with_simulation(&rpc_client, &payer_keypair, current_timestamp).await,
                "clmm" => clmm::test::test_clmm(&rpc_client, &payer_keypair, current_timestamp).await,
                "launchpad" => launchpad::test::test_launchpad(&rpc_client, current_timestamp).await,
                "stable_swap" => stable_swap::test::test_stable_swap(&rpc_client, current_timestamp).await,
                "damm_v1" => damm_v1::test::test_damm_v1_with_simulation(&rpc_client, &payer_keypair, current_timestamp).await,
                "damm_v2" => damm_v2::test::test_damm_v2_with_simulation(&rpc_client, &payer_keypair, current_timestamp).await,
                "dlmm" => dlmm::test::test_dlmm_with_simulation(&rpc_client, &payer_keypair, current_timestamp).await,
                "whirlpool" => whirlpool::test::test_whirlpool_with_simulation(&rpc_client, &payer_keypair, current_timestamp).await,
                "orca_amm_v2" => amm_v2::test::test_orca_amm_v2(&rpc_client, current_timestamp).await,
                "orca_amm_v1" => amm_v1::test::test_orca_amm_v1(&rpc_client, current_timestamp).await,
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