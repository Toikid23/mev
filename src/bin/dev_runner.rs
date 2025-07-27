// src/bin/dev_runner.rs
// (Garder tous les 'use' du fichier précédent)
use anyhow::{Result, anyhow};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;
use mev::{
    config::Config,
    decoders::{
        pool_operations::PoolOperations,
        raydium_decoders::{amm_v4, clmm_pool, cpmm, launchpad},
        meteora_decoders::{self, dlmm},
    },
};

// --- BANC D'ESSAI ---

async fn test_ammv4(rpc_client: &RpcClient) -> Result<()> {
    const POOL_ADDRESS: &str = "58oQChx4yWmvKdwLLZzBi4ChoCc2fqCUWBkwMihLYQo2"; // SOL-USDC
    println!("\n--- Test Raydium AMMv4 ({}) ---", POOL_ADDRESS);
    let pool_pubkey = Pubkey::from_str(POOL_ADDRESS)?;
    let account_data = rpc_client.get_account_data(&pool_pubkey).await?;
    let mut pool = amm_v4::decode_pool(&pool_pubkey, &account_data)?;

    println!("[1/2] Hydratation...");
    amm_v4::hydrate(&mut pool, rpc_client).await?;
    println!("-> Hydratation terminée. Frais: {:.4}%.", pool.fee_as_percent());

    let amount_in = 1_000_000_000; // 1 SOL
    println!("[2/2] Calcul du quote pour 1 SOL...");
    print_quote_result(&pool, &pool.mint_a, 9, 6, amount_in)
}

async fn test_clmm(rpc_client: &RpcClient) -> Result<()> {
    const POOL_ADDRESS: &str = "3ucNos4NbumPLZNWztqGHNFFgkHeRMBQAVemeeomsUxv"; // stSOL-SOL
    const PROGRAM_ID: &str = "CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK";
    println!("\n--- Test Raydium CLMM ({}) ---", POOL_ADDRESS);
    let pool_pubkey = Pubkey::from_str(POOL_ADDRESS)?;
    let program_pubkey = Pubkey::from_str(PROGRAM_ID)?;
    let account_data = rpc_client.get_account_data(&pool_pubkey).await?;
    let mut pool = clmm_pool::decode_pool(&pool_pubkey, &account_data, &program_pubkey)?;

    println!("[1/2] Hydratation...");
    clmm_pool::hydrate(&mut pool, rpc_client).await?;
    println!("-> Hydratation terminée. Frais: {:.4}%. Trouvé {} TickArray(s).", pool.fee_as_percent(), pool.tick_arrays.as_ref().unwrap().len());

    let amount_in = 1_000_000_000; // 1 stSOL
    println!("[2/2] Calcul du quote pour 1 stSOL...");
    print_quote_result(&pool, &pool.mint_a, pool.mint_a_decimals, pool.mint_b_decimals, amount_in)
}

async fn test_cpmm(rpc_client: &RpcClient) -> Result<()> {
    const POOL_ADDRESS: &str = "CGqPihUta62ZuJSctLx2mCEde6xJk8DL4c5UKiryE1pi"; // DUST-SOL
    println!("\n--- Test Raydium CPMM ({}) ---", POOL_ADDRESS);
    let pool_pubkey = Pubkey::from_str(POOL_ADDRESS)?;
    let account_data = rpc_client.get_account_data(&pool_pubkey).await?;
    let mut pool = cpmm::decode_pool(&pool_pubkey, &account_data)?;

    println!("[1/2] Hydratation...");
    cpmm::hydrate(&mut pool, rpc_client).await?;
    println!("-> Hydratation terminée. Frais: {:.4}%.", pool.fee_as_percent());

    let amount_in = 1_000_000_000; // 1 DUST
    println!("[2/2] Calcul du quote pour 1 DUST...");
    print_quote_result(&pool, &pool.token_0_mint, 9, 9, amount_in)
}

async fn test_launchpad(rpc_client: &RpcClient) -> Result<()> {
    const POOL_ADDRESS: &str = "HrVDirXSt8mDGwA5cperG58TKeAQGVuve5NtgCsWvLVp"; // ZETA-USDC
    println!("\n--- Test Raydium Launchpad ({}) ---", POOL_ADDRESS);
    let pool_pubkey = Pubkey::from_str(POOL_ADDRESS)?;
    let account_data = rpc_client.get_account_data(&pool_pubkey).await?;
    let mut pool = launchpad::decode_pool(&pool_pubkey, &account_data)?;

    println!("[1/2] Hydratation...");
    launchpad::hydrate(&mut pool, rpc_client).await?;
    println!("-> Hydratation terminée. Frais: {:.4}%. Courbe: {:?}", pool.fee_as_percent(), pool.curve_type);

    let amount_in = 1_000_000; // 1 USDC
    println!("[2/2] Calcul du quote pour 1 USDC...");
    print_quote_result(&pool, &pool.mint_b, 6, 6, amount_in)
}

async fn test_dlmm(rpc_client: &RpcClient) -> Result<()> {
    const POOL_ADDRESS: &str = "5rCf1DM8LjKTw4YqhnoLcngyZYeNnQqztScTogYHAS6"; // JUP-USDC
    const PROGRAM_ID: &str = "LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo";
    println!("\n--- Test Meteora DLMM ({}) ---", POOL_ADDRESS);
    let pool_pubkey = Pubkey::from_str(POOL_ADDRESS)?;
    let program_pubkey = Pubkey::from_str(PROGRAM_ID)?;
    let account_data = rpc_client.get_account_data(&pool_pubkey).await?;
    let mut pool = dlmm::decode_lb_pair(&pool_pubkey, &account_data, &program_pubkey)?;

    println!("[1/2] Hydratation...");
    dlmm::hydrate(&mut pool, rpc_client).await?;
    println!("-> Hydratation terminée. Frais: {:.4}%. Trouvé {} BinArray(s).",
        pool.fee_as_percent(),
        pool.hydrated_bin_arrays.as_ref().unwrap().len()
    );
    let amount_in = 1_000_000; // 1 USDC
    println!("[2/2] Calcul du quote pour 1 USDC...");
    print_quote_result(&pool, &pool.mint_b, 6, 6, amount_in)
}

#[tokio::main]
async fn main() -> Result<()> {
    println!("--- Lancement du banc d'essai complet des décodeurs ---");
    let config = Config::load()?;
    let rpc_client = RpcClient::new(config.solana_rpc_url);

    if let Err(e) = test_ammv4(&rpc_client).await { println!("!! AMMv4 a échoué: {}", e); }
    if let Err(e) = test_clmm(&rpc_client).await { println!("!! CLMM a échoué: {}", e); }
    if let Err(e) = test_cpmm(&rpc_client).await { println!("!! CPMM a échoué: {}", e); }
    if let Err(e) = test_launchpad(&rpc_client).await { println!("!! Launchpad a échoué: {}", e); }
    if let Err(e) = test_dlmm(&rpc_client).await { println!("!! DLMM a échoué: {}", e); }

    println!("\n--- Banc d'essai terminé ---");
    Ok(())
}

// --- UTILITAIRE D'AFFICHAGE AMÉLIORÉ ---
fn print_quote_result<T: PoolOperations + ?Sized>(
    pool: &T,
    token_in_mint: &Pubkey,
    in_decimals: u8,
    out_decimals: u8,
    amount_in: u64
) -> Result<()> {
    match pool.get_quote(token_in_mint, amount_in) {
        Ok(amount_out) => {
            let ui_amount_in = amount_in as f64 / 10f64.powi(in_decimals as i32);
            let ui_amount_out = amount_out as f64 / 10f64.powi(out_decimals as i32);
            let price = ui_amount_out / ui_amount_in;
            println!("-> Succès ! Pour {} UI, vous obtenez {} UI.", ui_amount_in, ui_amount_out);
            println!("-> Prix effectif: {:.6}", price);
        },
        Err(e) => {
            println!("!! Erreur lors du calcul du quote: {}", e);
            return Err(e);
        },
    }
    Ok(())
}