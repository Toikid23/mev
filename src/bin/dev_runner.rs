// src/bin/dev_runner.rs

use mev::config::Config;
use mev::data_pipeline::onchain_scanner;
use solana_client::rpc_client::RpcClient;
use anyhow::Result;

const RAYDIUM_LAUNCHPAD_PROGRAM_ID: &str = "LanMV9sAd7wArD4vJFi2qDdfnVhFxYSUg6eADduJ3uj";
const POOL_STATE_DISCRIMINATOR: [u8; 8] = [247, 237, 227, 245, 215, 195, 222, 70];

#[tokio::main]
async fn main() -> Result<()> {
    println!("--- Development Runner: DIAGNOSTIC de la taille des comptes Launchpad ---");

    let config = Config::load().expect("Erreur chargement config");
    let rpc_client = RpcClient::new(config.solana_rpc_url);

    let raw_accounts = onchain_scanner::find_pools_by_program_id(&rpc_client, RAYDIUM_LAUNCHPAD_PROGRAM_ID)?;

    println!("Scan réussi. {} comptes trouvés.", raw_accounts.len());

    println!("\nRecherche des comptes 'PoolState' et affichage de la taille de leurs données...");

    let mut found_count = 0;
    for account in raw_accounts.iter() {
        if account.data.len() > 8 && &account.data[..8] == POOL_STATE_DISCRIMINATOR {

            // --- C'EST LA LIGNE IMPORTANTE ---
            // On affiche la taille totale des données, et la taille après le discriminator.
            println!(
                "Compte PoolState trouvé ! Adresse: {}. Taille totale: {}. Taille des données: {}",
                account.address,
                account.data.len(),
                account.data.len() - 8
            );

            found_count += 1;
            // On s'arrête après en avoir trouvé quelques-uns pour ne pas surcharger.
            if found_count >= 5 {
                break;
            }
        }
    }

    if found_count == 0 {
        println!("\nAucun compte de type 'PoolState' n'a été trouvé.");
    }

    Ok(())
}