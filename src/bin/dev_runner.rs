// DANS : src/bin/dev_runner.rs

use anyhow::Result;
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;
use std::env;
use mev::{
    config::Config,
    decoders::{
        pool_operations::PoolOperations,
        raydium_decoders::{amm_v4, clmm_pool, cpmm, launchpad},
        meteora_decoders::dlmm,
    },
};


// =================================================================================
// DÉFINITIONS DES TESTS INDIVIDUELS
// Chaque test est une fonction `async` qui prend le RpcClient en argument.
// =================================================================================

async fn test_ammv4(rpc_client: &RpcClient) -> Result<()> {
    const POOL_ADDRESS: &str = "58oQChx4yWmvKdwLLZzBi4ChoCc2fqCUWBkwMihLYQo2"; // SOL-USDC
    println!("\n--- Test Raydium AMMv4 ({}) ---", POOL_ADDRESS);
    let pool_pubkey = Pubkey::from_str(POOL_ADDRESS)?;
    let account_data = rpc_client.get_account_data(&pool_pubkey).await?;
    let mut pool = amm_v4::decode_pool(&pool_pubkey, &account_data)?;

    println!("[1/4] Hydratation...");
    amm_v4::hydrate(&mut pool, rpc_client).await?;
    println!("-> Hydratation terminée. Frais: {:.4}%.", pool.fee_as_percent());

    // --- TEST DE VENTE DE SOL ---
    let sell_sol_amount = 1_000_000_000; // 1 SOL (9 décimales)
    println!("\n[2/4] Calcul pour VENDRE 1 SOL...");
    print_quote_result(&pool, &pool.mint_a, 9, 6, sell_sol_amount)?;

    // --- TEST D'ACHAT DE SOL ---
    // Pour acheter 1 SOL, il nous faut environ 186 USDC.
    // Nous allons simuler la vente de 187 USDC pour acheter du SOL.
    let sell_usdc_amount = 187_000_000; // 187 USDC (6 décimales)
    println!("\n[3/4] Calcul pour ACHETER du SOL avec 187 USDC...");
    // On inverse les paramètres : on donne le mint_b (USDC) en entrée.
    print_quote_result(&pool, &pool.mint_b, 6, 9, sell_usdc_amount)?;

    // --- TEST D'ACHAT DE SOL (GROS MONTANT) ---
    let large_sell_usdc_amount = 20_000_000_000; // 20,000 USDC (6 décimales)
    println!("\n[4/4] Calcul pour ACHETER du SOL avec 20,000 USDC...");
    print_quote_result(&pool, &pool.mint_b, 6, 9, large_sell_usdc_amount)?;

    Ok(())
}

async fn test_cpmm(rpc_client: &RpcClient) -> Result<()> {
    const POOL_ADDRESS: &str = "4XFxwYkzeXaaToxgdHoYE3powjJwZFy2bSk2Aq4bwFge"; // DUST-SOL
    println!("\n--- Test Raydium CPMM ({}) ---", POOL_ADDRESS);
    let pool_pubkey = Pubkey::from_str(POOL_ADDRESS)?;
    let account_data = rpc_client.get_account_data(&pool_pubkey).await?;
    let mut pool = cpmm::decode_pool(&pool_pubkey, &account_data)?;

    println!("[1/3] Hydratation...");
    cpmm::hydrate(&mut pool, rpc_client).await?;
    println!("-> Hydratation terminée. Frais: {:.4}%.", pool.fee_as_percent());

    // Test A : Petit montant
    let small_amount_in = 1_000_000_000; // 1 DUST (9 décimales)
    println!("\n[2/3] Calcul du quote pour 1 DUST...");
    print_quote_result(&pool, &pool.token_0_mint, 9, 9, small_amount_in)?;

    // Test B : Gros montant
    let large_amount_in = 100_000_000_000; // 100 DUST (9 décimales)
    println!("\n[3/3] Calcul du quote pour 100 DUST...");
    print_quote_result(&pool, &pool.token_0_mint, 9, 9, large_amount_in)?;

    Ok(())
}

async fn test_clmm(rpc_client: &RpcClient) -> Result<()> {
    const POOL_ADDRESS: &str = "3ucNos4NbumPLZNWztqGHNFFgkHeRMBQAVemeeomsUxv"; // stSOL-SOL
    const PROGRAM_ID: &str = "CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK";
    println!("\n--- Test Raydium CLMM ({}) ---", POOL_ADDRESS);
    let pool_pubkey = Pubkey::from_str(POOL_ADDRESS)?;
    let program_pubkey = Pubkey::from_str(PROGRAM_ID)?;
    let account_data = rpc_client.get_account_data(&pool_pubkey).await?;
    let mut pool = clmm_pool::decode_pool(&pool_pubkey, &account_data, &program_pubkey)?;

    println!("[1/3] Hydratation...");
    clmm_pool::hydrate(&mut pool, rpc_client).await?;
    println!("-> Hydratation terminée. Frais: {:.4}%. Trouvé {} TickArray(s).", pool.fee_as_percent(), pool.tick_arrays.as_ref().unwrap().len());

    // Test A : Petit montant
    let small_amount_in = 1_000_000_000; // 1 stSOL (9 décimales)
    println!("\n[2/3] Calcul du quote pour 1 stSOL...");
    print_quote_result(&pool, &pool.mint_a, pool.mint_a_decimals, pool.mint_b_decimals, small_amount_in)?;

    // Test B : Gros montant
    let large_amount_in = 100_000_000_000; // 100 stSOL (9 décimales)
    println!("\n[3/3] Calcul du quote pour 100 stSOL...");
    print_quote_result(&pool, &pool.mint_a, pool.mint_a_decimals, pool.mint_b_decimals, large_amount_in)?;

    Ok(())
}

async fn test_launchpad(rpc_client: &RpcClient) -> Result<()> {
    const POOL_ADDRESS: &str = "DReGGrVpi1Czq5tC1Juu2NjZh1jtack4GwckuJqbQd7H"; // ZETA-USDC
    println!("\n--- Test Raydium Launchpad ({}) ---", POOL_ADDRESS);
    let pool_pubkey = Pubkey::from_str(POOL_ADDRESS)?;
    let account_data = rpc_client.get_account_data(&pool_pubkey).await?;
    let mut pool = launchpad::decode_pool(&pool_pubkey, &account_data)?;

    println!("[1/3] Hydratation...");
    launchpad::hydrate(&mut pool, rpc_client).await?;
    println!("-> Hydratation terminée. Frais: {:.4}%. Courbe: {:?}", pool.fee_as_percent(), pool.curve_type);

    // Test A : Petit montant
    let small_amount_in = 1_000_000; // 1 USDC (6 décimales)
    println!("\n[2/3] Calcul du quote pour 1 USDC...");
    print_quote_result(&pool, &pool.mint_b, 6, 6, small_amount_in)?;

    // Test B : Gros montant
    let large_amount_in = 10_000_000_000; // 10,000 USDC (6 décimales)
    println!("\n[3/3] Calcul du quote pour 10,000 USDC...");
    print_quote_result(&pool, &pool.mint_b, 6, 6, large_amount_in)?;

    Ok(())
}


async fn test_dlmm(rpc_client: &RpcClient) -> Result<()> {
    const POOL_ADDRESS: &str = "5rCf1DM8LjKTw4YqhnoLcngyZYeNnQqztScTogYHAS6"; // WSOL-USDC
    const PROGRAM_ID: &str = "LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo";
    println!("\n--- Test Meteora DLMM ({}) ---", POOL_ADDRESS);
    let pool_pubkey = Pubkey::from_str(POOL_ADDRESS)?;
    let program_pubkey = Pubkey::from_str(PROGRAM_ID)?;
    let account_data = rpc_client.get_account_data(&pool_pubkey).await?;
    let mut pool = dlmm::decode_lb_pair(&pool_pubkey, &account_data, &program_pubkey)?;

    println!("[1/2] Hydratation...");
    dlmm::hydrate(&mut pool, rpc_client, 10).await?;
    println!("-> Hydratation terminée. Frais de base: {:.4}%. Trouvé {} BinArray(s).",
             pool.fee_as_percent(),
             pool.hydrated_bin_arrays.as_ref().unwrap().len()
    );

    let small_amount_in = 1_000_000; // 1 USDC (6 décimales)
    println!("\n[2/3] Calcul du quote pour 1 USDC...");
    print_quote_result(&pool, &pool.mint_b, 6, 9, small_amount_in)?;

    let large_amount_in = 20_000_000_000; // 20,000 USDC (6 décimales)
    println!("\n[3/3] Calcul du quote pour 20,000 USDC...");
    print_quote_result(&pool, &pool.mint_b, 6, 9, large_amount_in)?;

    Ok(())
}


// =================================================================================
// ORCHESTRATEUR DE TESTS
// Le `main` lit les arguments de la ligne de commande et lance les tests demandés.
// =================================================================================

#[tokio::main]
async fn main() -> Result<()> {
    println!("--- Lancement du Banc d'Essai des Décodeurs ---");
    let config = Config::load()?;
    let rpc_client = RpcClient::new(config.solana_rpc_url);

    // On récupère les arguments passés en ligne de commande (ex: "amm_v4", "clmm")
    let args: Vec<String> = env::args().skip(1).collect();

    if args.is_empty() {
        // Si aucun argument n'est donné, on lance TOUS les tests
        println!("Mode: Exécution de tous les tests.");
        if let Err(e) = test_ammv4(&rpc_client).await { println!("!! AMMv4 a échoué: {}", e); }
        if let Err(e) = test_cpmm(&rpc_client).await { println!("!! CPMM a échoué: {}", e); }
        if let Err(e) = test_clmm(&rpc_client).await { println!("!! CLMM a échoué: {}", e); }
        if let Err(e) = test_launchpad(&rpc_client).await { println!("!! Launchpad a échoué: {}", e); }
        if let Err(e) = test_dlmm(&rpc_client).await { println!("!! DLMM a échoué: {}", e); }
    } else {
        // Sinon, on lance uniquement les tests demandés
        println!("Mode: Exécution des tests spécifiques: {:?}", args);
        for test_name in args {
            match test_name.as_str() {
                "ammv4" => if let Err(e) = test_ammv4(&rpc_client).await { println!("!! AMMv4 a échoué: {}", e); },
                "cpmm" => if let Err(e) = test_cpmm(&rpc_client).await { println!("!! CPMM a échoué: {}", e); },
                "clmm" => if let Err(e) = test_clmm(&rpc_client).await { println!("!! CLMM a échoué: {}", e); },
                "launchpad" => if let Err(e) = test_launchpad(&rpc_client).await { println!("!! Launchpad a échoué: {}", e); },
                "dlmm" => if let Err(e) = test_dlmm(&rpc_client).await { println!("!! DLMM a échoué: {}", e); },
                _ => println!("!! Test inconnu: '{}'", test_name),
            }
        }
    }

    println!("\n--- Banc d'essai terminé ---");
    Ok(())
}

// =================================================================================
// FONCTION UTILITAIRE D'AFFICHAGE (INCHANGÉE)
// =================================================================================

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
            let price = if ui_amount_in > 0.0 { ui_amount_out / ui_amount_in } else { 0.0 };
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