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
        meteora_decoders::{dlmm, amm as meteora_amm}
    },
};
use mev::decoders::orca_decoders::{whirlpool_decoder, token_swap_v2, token_swap_v1};

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

    println!("[1/2] Hydratation...");
    amm_v4::hydrate(&mut pool, rpc_client).await?;
    println!("-> Hydratation terminée. Frais: {:.4}%.", pool.fee_as_percent());

    let sell_sol_amount = 1_000_000_000; // 1 SOL (9 décimales)
    println!("\n[2/2] Calcul pour VENDRE 1 SOL...");
    print_quote_result(&pool, &pool.mint_a, 9, 6, sell_sol_amount)?;

    Ok(())
}

async fn test_cpmm(rpc_client: &RpcClient) -> Result<()> {
    const POOL_ADDRESS: &str = "4XFxwYkzeXaaToxgdHoYE3powjJwZFy2bSk2Aq4bwFge"; // DUST-SOL
    println!("\n--- Test Raydium CPMM ({}) ---", POOL_ADDRESS);
    let pool_pubkey = Pubkey::from_str(POOL_ADDRESS)?;
    let account_data = rpc_client.get_account_data(&pool_pubkey).await?;
    let mut pool = cpmm::decode_pool(&pool_pubkey, &account_data)?;

    println!("[1/2] Hydratation...");
    cpmm::hydrate(&mut pool, rpc_client).await?;
    println!("-> Hydratation terminée. Frais: {:.4}%.", pool.fee_as_percent());

    let small_amount_in = 1_000_000_000; // 1 DUST (9 décimales)
    println!("\n[2/2] Calcul du quote pour 1 DUST...");
    print_quote_result(&pool, &pool.token_0_mint, 9, 9, small_amount_in)?;

    Ok(())
}

async fn test_clmm(rpc_client: &RpcClient) -> Result<()> {
    const POOL_ADDRESS: &str = "3ucNos4NbumPLZNWztqGHNFFgkHeRMBQAVemeeomsUxv"; // SOL-USDC
    const PROGRAM_ID: &str = "CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK";
    println!("\n--- Test Raydium CLMM ({}) ---", POOL_ADDRESS);
    let pool_pubkey = Pubkey::from_str(POOL_ADDRESS)?;
    let program_pubkey = Pubkey::from_str(PROGRAM_ID)?;
    let account_data = rpc_client.get_account_data(&pool_pubkey).await?;
    let mut pool = clmm_pool::decode_pool(&pool_pubkey, &account_data, &program_pubkey)?;

    println!("[1/2] Hydratation...");
    clmm_pool::hydrate(&mut pool, rpc_client).await?;
    println!("-> Hydratation terminée. Frais: {:.4}%. Trouvé {} TickArray(s).", pool.fee_as_percent(), pool.tick_arrays.as_ref().unwrap().len());

    let small_amount_in = 1_000_000_000; // 1 SOL (9 décimales)
    println!("\n[2/2] Calcul du quote pour 1 SOL...");
    print_quote_result(&pool, &pool.mint_a, pool.mint_a_decimals, pool.mint_b_decimals, small_amount_in)?;

    Ok(())
}

async fn test_launchpad(rpc_client: &RpcClient) -> Result<()> {
    const POOL_ADDRESS: &str = "DReGGrVpi1Czq5tC1Juu2NjZh1jtack4GwckuJqbQd7H"; // ZETA-USDC
    println!("\n--- Test Raydium Launchpad ({}) ---", POOL_ADDRESS);
    let pool_pubkey = Pubkey::from_str(POOL_ADDRESS)?;
    let account_data = rpc_client.get_account_data(&pool_pubkey).await?;
    let mut pool = launchpad::decode_pool(&pool_pubkey, &account_data)?;

    println!("[1/2] Hydratation...");
    launchpad::hydrate(&mut pool, rpc_client).await?;
    println!("-> Hydratation terminée. Frais: {:.4}%. Courbe: {:?}", pool.fee_as_percent(), pool.curve_type);

    let small_amount_in = 1_000_000; // 1 USDC (6 décimales)
    println!("\n[2/2] Calcul du quote pour 1 USDC...");
    print_quote_result(&pool, &pool.mint_b, 6, 6, small_amount_in)?;

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

    let small_amount_in = 183_560_000; // 1 USDC (6 décimales)
    println!("\n[2/2] Calcul du quote pour 1 USDC...");
    print_quote_result(&pool, &pool.mint_b, 6, 9, small_amount_in)?;

    Ok(())
}



/*async fn test_whirlpool(rpc_client: &RpcClient) -> Result<()> {
    const POOL_ADDRESS: &str = "Czfq3xZZDmsdGdUyrNLtRhGc47cXcZtLG4crryfu44zE"; // WSOL-USDC

    println!("\n--- Test Orca Whirlpool ({}) ---", POOL_ADDRESS);

    let pool_pubkey = Pubkey::from_str(POOL_ADDRESS)?;

    // 1. Décodage initial à partir des données du compte
    let account_data = rpc_client.get_account_data(&pool_pubkey).await?;
    let mut pool = whirlpool_decoder::decode_pool(&pool_pubkey, &account_data)?;
    println!("-> Décodage initial réussi.");

    // 2. Hydratation pour charger les données annexes (TickArrays, Mints)
    println!("[1/2] Hydratation...");
    whirlpool_decoder::hydrate(&mut pool, rpc_client).await?;
    println!("-> Hydratation terminée. Frais: {:.4}%. Trouvé {} TickArray(s).",
             pool.fee_as_percent(),
             pool.tick_arrays.as_ref().unwrap().len());

    // 3. Simulation d'un swap avec `get_quote`
    let usdc_amount_in = 1_000_000; // 1 USDC (a 6 décimales)
    println!("\n[2/2] Calcul du quote pour 1 USDC...");

    // On utilise les décimales hydratées pour un affichage correct.
    // Pour WSOL/USDC, USDC est généralement le `mint_b`.
    print_quote_result(
        &pool,
        &pool.mint_b, // On entre du USDC (mint_b)
        pool.mint_b_decimals, // Décimales de l'input
        pool.mint_a_decimals, // Décimales de l'output (WSOL)
        usdc_amount_in
    )?;

    Ok(())
}*/


async fn test_whirlpool(rpc_client: &RpcClient) -> Result<()> {
    const POOL_ADDRESS: &str = "Czfq3xZZDmsdGdUyrNLtRhGc47cXcZtLG4crryfu44zE"; // WSOL-USDC
    println!("\n--- Test Orca Whirlpool ({}) ---", POOL_ADDRESS);

    let pool_pubkey = Pubkey::from_str(POOL_ADDRESS)?;

    // 1. Décodage initial
    let account_data = rpc_client.get_account_data(&pool_pubkey).await?;
    let mut pool = whirlpool_decoder::decode_pool(&pool_pubkey, &account_data)?;
    println!("-> Décodage initial réussi (tick_current: {}, tick_spacing: {}).", pool.tick_current_index, pool.tick_spacing);

    // 2. Hydratation
    println!("[1/2] Hydratation...");
    whirlpool_decoder::hydrate(&mut pool, rpc_client).await?;

    let tick_arrays_found = pool.tick_arrays.as_ref().map_or(0, |arrays| arrays.len());
    println!("-> Hydratation terminée. Frais: {:.4}%. Trouvé {} TickArray(s).",
             pool.fee_as_percent(),
             tick_arrays_found);

    if tick_arrays_found == 0 {
        println!("\n[AVERTISSEMENT] Aucun TickArray trouvé. Le calcul de quote sera 0.");
    }

    // 3. Simulation d'un swap
    let usdc_amount_in = 1_000_000; // 1 USDC
    println!("\n[2/2] Calcul du quote pour 1 USDC...");

    print_quote_result(
        &pool,
        &pool.mint_b,
        pool.mint_b_decimals,
        pool.mint_a_decimals,
        usdc_amount_in
    )?;

    Ok(())
}


async fn test_orca_amm_v1(rpc_client: &RpcClient) -> Result<()> {
    // VEUILLEZ REMPLACER CECI PAR UNE ADRESSE DE POOL ORCA AMM V1
    const POOL_ADDRESS: &str = "6fTRDD7sYxCN7oyoSQaN1AWC3P2m8A6gVZzGrpej9DvL";
    println!("\n--- Test Orca AMM V1 ({}) ---", POOL_ADDRESS);

    let pool_pubkey = Pubkey::from_str(POOL_ADDRESS)?;

    // 1. Décodage initial à partir des données brutes
    let account_data = rpc_client.get_account_data(&pool_pubkey).await?;
    let mut pool = token_swap_v1::decode_pool(&pool_pubkey, &account_data)?;
    println!("-> Décodage initial V1 réussi. Courbe type: {}.", pool.curve_type);

    // 2. Hydratation pour charger les réserves et les frais de transfert
    println!("[1/2] Hydratation V1...");
    token_swap_v1::hydrate(&mut pool, rpc_client).await?;
    println!("-> Hydratation V1 terminée. Frais: {:.4}%.", pool.fee_as_percent());
    println!("   -> Réserve A ({}) : {} (décimales: {})", pool.mint_a, pool.reserve_a, pool.mint_a_decimals);
    println!("   -> Réserve B ({}) : {} (décimales: {})", pool.mint_b, pool.reserve_b, pool.mint_b_decimals);

    // 3. Simulation d'un swap avec `get_quote`
    // On va simuler un swap de 1 unité du token A.
    let amount_in = 1 * 10u64.pow(pool.mint_a_decimals as u32);
    println!("\n[2/2] Calcul du quote pour 1 Token A...");

    print_quote_result(
        &pool,
        &pool.mint_a,         // On entre du token A
        pool.mint_a_decimals, // Décimales de l'input
        pool.mint_b_decimals, // Décimales de l'output
        amount_in
    )?;

    Ok(())
}


async fn test_orca_amm_v2(rpc_client: &RpcClient) -> Result<()> {
    // REMPLACEZ CECI PAR UNE ADRESSE DE POOL ORCA AMM V2 (PAS WHIRLPOOL)
    // Exemple de format (ce n'est PAS une vraie adresse) : "Fk9y...p8s"
    const POOL_ADDRESS: &str = "EGZ7tiLeH62TPV1gL8WwbXGzEPa9zmcpVnnkPKKnrE2U"; // Exemple : SAMO/USDC
    println!("\n--- Test Orca AMM V2 ({}) ---", POOL_ADDRESS);

    let pool_pubkey = Pubkey::from_str(POOL_ADDRESS)?;

    // 1. Décodage initial à partir des données brutes
    let account_data = rpc_client.get_account_data(&pool_pubkey).await?;
    let mut pool = token_swap_v2::decode_pool(&pool_pubkey, &account_data)?;
    println!("-> Décodage initial réussi. Courbe type: {}.", pool.curve_type);

    // 2. Hydratation pour charger les réserves et les frais de transfert
    println!("[1/2] Hydratation...");
    token_swap_v2::hydrate(&mut pool, rpc_client).await?;
    println!("-> Hydratation terminée. Frais: {:.4}%.", pool.fee_as_percent());
    println!("   -> Réserve A ({}): {} (décimales: {})", pool.mint_a, pool.reserve_a, pool.mint_a_decimals);
    println!("   -> Réserve B ({}): {} (décimales: {})", pool.mint_b, pool.reserve_b, pool.mint_b_decimals);

    // 3. Simulation d'un swap avec `get_quote`
    let amount_in = 1_000_000; // 1 USDC si USDC a 6 décimales
    println!("\n[2/2] Calcul du quote pour 1 USDC (en supposant que c'est le token B)...");

    // On utilise les décimales hydratées pour un affichage correct.
    // Pour SAMO/USDC, USDC est généralement le `mint_b`.
    print_quote_result(
        &pool,
        &pool.mint_b,         // On entre du USDC (mint_b)
        pool.mint_b_decimals, // Décimales de l'input (USDC)
        pool.mint_a_decimals, // Décimales de l'output (SAMO)
        amount_in as u64
    )?;

    Ok(())
}


async fn test_meteora_amm(rpc_client: &RpcClient) -> Result<()> {
    //
    // <<< MODIFIEZ L'ADRESSE ICI POUR CHANGER DE POOL À TESTER >>>
    //
    // const POOL_ADDRESS: &str = "32D4zRxNc1EssbJieVHfPhZM3rH6CzfUPrWUuWxD9prG"; // Stable (USDC-USDT)
    const POOL_ADDRESS: &str = "7rQd8FhC1rimV3v9edCRZ6RNFsJN1puXM9UmjaURJRNj"; // Constant Product (WSOL-USELESS)

    println!("\n--- Test Meteora AMM ({}) ---", POOL_ADDRESS);
    let pool_pubkey = Pubkey::from_str(POOL_ADDRESS)?;
    let account_data = rpc_client.get_account_data(&pool_pubkey).await?;

    let mut pool = meteora_amm::decode_pool(&pool_pubkey, &account_data)?;
    println!("-> Décodage initial réussi. Courbe détectée: {:?}", pool.curve_type);

    println!("[1/2] Hydratation...");
    meteora_amm::hydrate(&mut pool, rpc_client).await?;

    // CORRECTION FINALE : Calculer et afficher le VRAI pourcentage de frais payé par l'utilisateur.
    let fee_percent = (pool.fees.trade_fee_numerator as f64 * 100.0) / pool.fees.trade_fee_denominator as f64;
    println!("-> Hydratation terminée. Frais de trading: {:.4}%.", fee_percent);

    // Le test s'adapte automatiquement au pool testé.
    let amount_in = 1 * 10u64.pow(pool.mint_a_decimals as u32);
    let token_in_name = if POOL_ADDRESS == "32D4zRxNc1EssbJieVHfPhZM3rH6CzfUPrWUuWxD9prG" {
        "USDC"
    } else {
        "Token A (ex: WSOL)"
    };

    println!("\n[2/2] Calcul du quote pour 1 {}...", token_in_name);
    print_quote_result(&pool, &pool.mint_a, pool.mint_a_decimals, pool.mint_b_decimals, amount_in)?;

    Ok(())
}


// =================================================================================
// ORCHESTRATEUR DE TESTS
// =================================================================================

#[tokio::main]
async fn main() -> Result<()> {
    println!("--- Lancement du Banc d'Essai des Décodeurs ---");
    let config = Config::load()?;
    let rpc_client = RpcClient::new(config.solana_rpc_url);

    let args: Vec<String> = env::args().skip(1).collect();

    if args.is_empty() {
        println!("Mode: Exécution de tous les tests.");
        if let Err(e) = test_ammv4(&rpc_client).await { println!("!! AMMv4 a échoué: {}", e); }
        if let Err(e) = test_cpmm(&rpc_client).await { println!("!! CPMM a échoué: {}", e); }
        if let Err(e) = test_clmm(&rpc_client).await { println!("!! CLMM a échoué: {}", e); }
        if let Err(e) = test_launchpad(&rpc_client).await { println!("!! Launchpad a échoué: {}", e); }
        if let Err(e) = test_meteora_amm(&rpc_client).await { println!("!! Meteora AMM a échoué: {}", e); }
        if let Err(e) = test_dlmm(&rpc_client).await { println!("!! DLMM a échoué: {}", e); }
        if let Err(e) = test_whirlpool(&rpc_client).await { println!("!! Whirlpool a échoué: {}", e); }
        if let Err(e) = test_orca_amm_v2(&rpc_client).await { println!("!! Orca AMM V2 a échoué: {}", e); }
        if let Err(e) = test_orca_amm_v1(&rpc_client).await { println!("!! Orca AMM V1 a échoué: {}", e); }
    } else {
        println!("Mode: Exécution des tests spécifiques: {:?}", args);
        for test_name in args {
            match test_name.as_str() {
                "ammv4" => if let Err(e) = test_ammv4(&rpc_client).await { println!("!! AMMv4 a échoué: {}", e); },
                "cpmm" => if let Err(e) = test_cpmm(&rpc_client).await { println!("!! CPMM a échoué: {}", e); },
                "clmm" => if let Err(e) = test_clmm(&rpc_client).await { println!("!! CLMM a échoué: {}", e); },
                "launchpad" => if let Err(e) = test_launchpad(&rpc_client).await { println!("!! Launchpad a échoué: {}", e); },
                "meteora_amm" => if let Err(e) = test_meteora_amm(&rpc_client).await { println!("!! Meteora AMM a échoué: {}", e); },
                "dlmm" => if let Err(e) = test_dlmm(&rpc_client).await { println!("!! DLMM a échoué: {}", e); },
                "whirlpool" => if let Err(e) = test_whirlpool(&rpc_client).await { println!("!! Whirlpool a échoué: {}", e); },
                "orca_amm_v2" => if let Err(e) = test_orca_amm_v2(&rpc_client).await { println!("!! Orca AMM V2 a échoué: {}", e); },
                "orca_amm_v1" => if let Err(e) = test_orca_amm_v1(&rpc_client).await { println!("!! Orca AMM V1 a échoué: {}", e); },
                _ => println!("!! Test inconnu: '{}'", test_name),
            }
        }
    }

    println!("\n--- Banc d'essai terminé ---");
    Ok(())
}




// =================================================================================
// FONCTION UTILITAIRE D'AFFICHAGE
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
            return Err(e.into());
        },
    }
    Ok(())
}