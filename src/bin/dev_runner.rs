// Fichier COMPLET et FINAL : src/bin/dev_runner.rs

use solana_client::rpc_client::RpcClient;
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;
use mev::{
    config::Config,
    decoders::pool_operations::PoolOperations,
    decoders::raydium_decoders::clmm_pool,
};

const TARGET_POOL_ADDRESS: &str = "DgGC1qXjFQjKKYA6n3MVACMvtfXjRCdRqL674FhzSdAb"; // WSOL-Clarus

const RAYDIUM_CLMM_PROGRAM_ID: &str = "CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("--- Test Final: Calcul de Quote via CLMM ---");

    let config = Config::load()?;
    let rpc_client = RpcClient::new(config.solana_rpc_url);
    let pool_pubkey = Pubkey::from_str(TARGET_POOL_ADDRESS)?;
    let program_id = Pubkey::from_str(RAYDIUM_CLMM_PROGRAM_ID)?;

    println!("\n[1/3] Décodage du PoolState brut...");
    let pool_account_data = rpc_client.get_account_data(&pool_pubkey)?;
    let mut decoded_pool = clmm_pool::decode_pool(&pool_pubkey, &pool_account_data, &program_id)?;
    println!("-> Décodage initial réussi.");

    println!("\n[2/3] Hydratation complète du pool (frais, décimales, TickArrays)...");
    clmm_pool::hydrate(&mut decoded_pool, &rpc_client)?;
    println!("-> Hydratation terminée. Trouvé {} TickArray(s).", decoded_pool.tick_arrays.as_ref().unwrap().len());

    println!("\n[3/3] Calcul du quote...");
    let amount_in_sol: u64 = 1_000_000_000; // 1 SOL
    match decoded_pool.get_quote(&decoded_pool.mint_a, amount_in_sol) {
        Ok(amount_out) => {
            let amount_in_ui = amount_in_sol as f64 / 10f64.powi(decoded_pool.mint_a_decimals as i32);
            let amount_out_ui = amount_out as f64 / 10f64.powi(decoded_pool.mint_b_decimals as i32);
            let price = amount_out_ui / amount_in_ui;
            println!("-> Pour {} SOL, vous obtenez {} Clarus.", amount_in_ui, amount_out_ui);
            println!("-> Prix approximatif: {:.4} Clarus/SOL", price);
        },
        Err(e) => println!("!! Erreur lors du calcul du quote: {}", e),
    }

    println!("\n--- Test terminé ---");
    Ok(())
}