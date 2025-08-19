use anyhow::Result;
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;

// --- Imports depuis notre propre crate ---
use crate::decoders::PoolOperations;
use crate::decoders::orca::amm_v1::{
    decode_pool,
    hydrate,
};

// --- Fonction utilitaire de test (copiée depuis dev_runner.rs) ---
fn print_quote_result<T: PoolOperations + ?Sized>(
    pool: &T,
    token_in_mint: &Pubkey,
    in_decimals: u8,
    out_decimals: u8,
    amount_in: u64,
    current_timestamp: i64
) -> Result<()> {
    match pool.get_quote(token_in_mint, amount_in, current_timestamp) {
        Ok(amount_out) => {
            let ui_amount_in = amount_in as f64 / 10f64.powi(in_decimals as i32);
            let ui_amount_out = amount_out as f64 / 10f64.powi(out_decimals as i32);
            let price = if ui_amount_in > 0.0 { ui_amount_out / ui_amount_in } else { 0.0 };
            println!("-> Succès ! Pour {} UI du token d'entrée, vous obtenez {} UI du token de sortie.", ui_amount_in, ui_amount_out);
            println!("   Token d'entrée: {}", token_in_mint);
            println!("-> Prix effectif: {:.9}", price);
        },
        Err(e) => {
            println!("!! Erreur lors du calcul du quote: {}", e);
            return Err(e.into());
        },
    }
    Ok(())
}


// --- Votre fonction de test (rendue publique) ---
pub async fn test_orca_amm_v1(rpc_client: &RpcClient, current_timestamp: i64) -> Result<()> {
    const POOL_ADDRESS: &str = "6fTRDD7sYxCN7oyoSQaN1AWC3P2m8A6gVZzGrpej9DvL"; // SOL/USDC
    println!("\n--- Test Orca AMM V1 ({}) ---", POOL_ADDRESS);

    let pool_pubkey = Pubkey::from_str(POOL_ADDRESS)?;

    let account_data = rpc_client.get_account_data(&pool_pubkey).await?;
    let mut pool = decode_pool(&pool_pubkey, &account_data)?;
    println!("-> Décodage initial réussi. Courbe type: {}.", pool.curve_type);

    println!("[1/2] Hydratation...");
    hydrate(&mut pool, rpc_client).await?;
    println!("-> Hydratation terminée. Frais: {:.4}%.", pool.fee_as_percent());
    println!("   -> Réserve A ({}): {} (décimales: {})", pool.mint_a, pool.reserve_a, pool.mint_a_decimals);
    println!("   -> Réserve B ({}): {} (décimales: {})", pool.mint_b, pool.reserve_b, pool.mint_b_decimals);

    let amount_in_sol = 1_000_000_000;
    println!("\n[2/2] Calcul pour VENDRE 1 SOL...");

    print_quote_result(
        &pool,
        &pool.mint_a,
        pool.mint_a_decimals,
        pool.mint_b_decimals,
        amount_in_sol,
        current_timestamp
    )?;

    Ok(())
}