// DANS : src/bin/dev_runner.rs
// VERSION CORRIGÉE ET AMÉLIORÉE

use anyhow::{anyhow, Result};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;
use std::env;
use mev::{
    config::Config,
    decoders::{
        pool_operations::PoolOperations,
        raydium_decoders::{amm_v4, clmm_pool, cpmm, launchpad},
        meteora_decoders::{dlmm, amm as meteora_amm, damm_v2},
        pump_decoders
    },
};
use mev::decoders::orca_decoders::{whirlpool_decoder, token_swap_v2, token_swap_v1};


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
            println!("-> Succès ! Pour {} UI du token d'entrée, vous obtenez {} UI du token de sortie.", ui_amount_in, ui_amount_out);
            println!("   Token d'entrée: {}", token_in_mint);
            println!("-> Prix effectif: {:.6}", price);
        },
        Err(e) => {
            println!("!! Erreur lors du calcul du quote: {}", e);
            return Err(e.into());
        },
    }
    Ok(())
}


// =================================================================================
// DÉFINITIONS DES TESTS INDIVIDUELS (AVEC CORRECTIONS)
// =================================================================================


// --- TEST DLMM (CORRIGÉ) ---
// Cette fonction est maintenant un orchestrateur qui peut lancer plusieurs scénarios
// de test pour le même type de pool.
async fn test_dlmm(rpc_client: &RpcClient) -> Result<()> {
    // Étape 1: Définir les scénarios de test. C'est ici que vous pouvez facilement ajouter de nouveaux pools.
    struct DlmmTestCase<'a> {
        pool_address: &'a str,
        input_token_mint: &'a str,
        input_amount_ui: f64,
        description: &'a str,
    }

    let test_cases = vec![
        DlmmTestCase {
            pool_address: "5rCf1DM8LjKTw4YqhnoLcngyZYeNnQqztScTogYHAS6", // WSOL-USDC
            input_token_mint: "So11111111111111111111111111111111111111112", // WSOL
            input_amount_ui: 1.0,
            description: "WSOL-USDC (Vente de 1 WSOL)",
        },
        DlmmTestCase {
            pool_address: "9KUSnaqA7oWRfhHLut63aFCSdmybyFgnQBeJm9yudZXZ", // ANI-WSOL
            input_token_mint: "So11111111111111111111111111111111111111112", // WSOL
            input_amount_ui: 1.0,
            description: "ANI-WSOL (Vente de 1 WSOL)",
        },
        DlmmTestCase {
            pool_address: "9KUSnaqA7oWRfhHLut63aFCSdmybyFgnQBeJm9yudZXZ", // ANI-WSOL
            input_token_mint: "9tqjeRS1swj36Ee5C1iGiwAxjQJNGAVCzaTLwFY8bonk", // ANI
            input_amount_ui: 1000.0,
            description: "ANI-WSOL (Vente de 1000 ANI)",
        },
    ];

    const PROGRAM_ID: &str = "LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo";
    let program_pubkey = Pubkey::from_str(PROGRAM_ID)?;

    // Étape 2: Boucler et exécuter chaque scénario
    for (i, case) in test_cases.iter().enumerate() {
        println!("\n--- [DLMM Cas {}/{}] Test sur {} ---", i + 1, test_cases.len(), case.description);
        let pool_pubkey = Pubkey::from_str(case.pool_address)?;
        let input_mint_pubkey = Pubkey::from_str(case.input_token_mint)?;

        // Décodage
        let account_data = rpc_client.get_account_data(&pool_pubkey).await?;
        let mut pool = dlmm::decode_lb_pair(&pool_pubkey, &account_data, &program_pubkey)?;

        // Hydratation (aucune valeur en dur)
        println!("[1/2] Hydratation...");
        dlmm::hydrate(&mut pool, rpc_client, 10).await?;
        println!("-> Hydratation terminée. Trouvé {} BinArray(s).",
                 pool.hydrated_bin_arrays.as_ref().unwrap_or(&Default::default()).len());
        println!("   -> Mint A ({} décimales): {}", pool.mint_a_decimals, pool.mint_a);
        println!("   -> Mint B ({} décimales): {}", pool.mint_b_decimals, pool.mint_b);

        // Préparation du calcul (aucune valeur en dur)
        println!("\n[2/2] Calcul pour VENDRE {} UI de {}...", case.input_amount_ui, case.input_token_mint);

        // Logique dynamique pour trouver les bonnes décimales
        let (input_mint_decimals, output_mint_decimals) = if input_mint_pubkey == pool.mint_a {
            (pool.mint_a_decimals, pool.mint_b_decimals)
        } else if input_mint_pubkey == pool.mint_b {
            (pool.mint_b_decimals, pool.mint_a_decimals)
        } else {
            return Err(anyhow!("Le token d'input {} n'appartient pas au pool {}", case.input_token_mint, case.pool_address));
        };

        // Calcul du montant en unités de base
        let amount_in_base_units = (case.input_amount_ui * 10f64.powi(input_mint_decimals as i32)) as u64;

        // Appel à la fonction de quote avec les bons paramètres
        print_quote_result(
            &pool,
            &input_mint_pubkey,
            input_mint_decimals,
            output_mint_decimals,
            amount_in_base_units
        )?;
    }

    Ok(())
}


// --- AUTRES TESTS (inchangés, mais pourraient bénéficier du même modèle de scénarios) ---

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
    // Pour ce pool, mint_a=SOL(9) et mint_b=USDC(6)
    print_quote_result(&pool, &pool.mint_a, 9, 6, sell_sol_amount)?;

    Ok(())
}

async fn test_cpmm(rpc_client: &RpcClient) -> Result<()> {
    const POOL_ADDRESS: &str = "Q2sPHPdUWFMg7M7wwrQKLrn619cAucfRsmhVJffodSp"; // DUST-SOL
    println!("\n--- Test Raydium CPMM ({}) ---", POOL_ADDRESS);
    let pool_pubkey = Pubkey::from_str(POOL_ADDRESS)?;
    let account_data = rpc_client.get_account_data(&pool_pubkey).await?;
    let mut pool = cpmm::decode_pool(&pool_pubkey, &account_data)?;

    println!("[1/2] Hydratation...");
    cpmm::hydrate(&mut pool, rpc_client).await?;
    println!("-> Hydratation terminée. Frais: {:.4}%.", pool.fee_as_percent());

    let small_amount_in = 1_000_000_000; // 1 DUST (9 décimales)
    println!("\n[2/2] Calcul du quote pour 1 DUST...");
    // Pour ce pool, DUST(9) est token_0 et SOL(9) est token_1
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

async fn test_whirlpool(rpc_client: &RpcClient) -> Result<()> {
    const POOL_ADDRESS: &str = "Czfq3xZZDmsdGdUyrNLtRhGc47cXcZtLG4crryfu44zE"; // WSOL-USDC
    println!("\n--- Test Orca Whirlpool ({}) ---", POOL_ADDRESS);

    let pool_pubkey = Pubkey::from_str(POOL_ADDRESS)?;

    let account_data = rpc_client.get_account_data(&pool_pubkey).await?;
    let mut pool = whirlpool_decoder::decode_pool(&pool_pubkey, &account_data)?;
    println!("-> Décodage initial réussi (tick_current: {}, tick_spacing: {}).", pool.tick_current_index, pool.tick_spacing);

    println!("[1/2] Hydratation...");
    whirlpool_decoder::hydrate(&mut pool, rpc_client).await?;

    let tick_arrays_found = pool.tick_arrays.as_ref().map_or(0, |arrays| arrays.len());
    println!("-> Hydratation terminée. Frais: {:.4}%. Trouvé {} TickArray(s).",
             pool.fee_as_percent(),
             tick_arrays_found);

    if tick_arrays_found == 0 {
        println!("\n[AVERTISSEMENT] Aucun TickArray trouvé. Le calcul de quote sera 0.");
    }

    let sol_amount_in = 1_000_000_000; // 1 SOL
    println!("\n[2/2] Calcul pour VENDRE 1 SOL...");

    print_quote_result(
        &pool,
        &pool.mint_a,         // On entre du SOL (mint_a)
        pool.mint_a_decimals,
        pool.mint_b_decimals,
        sol_amount_in
    )?;

    Ok(())
}

async fn test_orca_amm_v1(rpc_client: &RpcClient) -> Result<()> {
    const POOL_ADDRESS: &str = "6fTRDD7sYxCN7oyoSQaN1AWC3P2m8A6gVZzGrpej9DvL"; // SAMO-USDC
    println!("\n--- Test Orca AMM V1 ({}) ---", POOL_ADDRESS);

    let pool_pubkey = Pubkey::from_str(POOL_ADDRESS)?;

    let account_data = rpc_client.get_account_data(&pool_pubkey).await?;
    let mut pool = token_swap_v1::decode_pool(&pool_pubkey, &account_data)?;
    println!("-> Décodage initial V1 réussi. Courbe type: {}.", pool.curve_type);

    println!("[1/2] Hydratation V1...");
    token_swap_v1::hydrate(&mut pool, rpc_client).await?;
    println!("-> Hydratation V1 terminée. Frais: {:.4}%.", pool.fee_as_percent());
    println!("   -> Réserve A ({}) : {} (décimales: {})", pool.mint_a, pool.reserve_a, pool.mint_a_decimals);
    println!("   -> Réserve B ({}) : {} (décimales: {})", pool.mint_b, pool.reserve_b, pool.mint_b_decimals);

    let amount_in = 1 * 10u64.pow(pool.mint_a_decimals as u32);
    println!("\n[2/2] Calcul du quote pour 1 Token A...");

    print_quote_result(
        &pool,
        &pool.mint_a,
        pool.mint_a_decimals,
        pool.mint_b_decimals,
        amount_in
    )?;

    Ok(())
}

async fn test_orca_amm_v2(rpc_client: &RpcClient) -> Result<()> {
    const POOL_ADDRESS: &str = "EGZ7tiLeH62TPV1gL8WwbXGzEPa9zmcpVnnkPKKnrE2U"; // SOL/USDC
    println!("\n--- Test Orca AMM V2 ({}) ---", POOL_ADDRESS);

    let pool_pubkey = Pubkey::from_str(POOL_ADDRESS)?;

    let account_data = rpc_client.get_account_data(&pool_pubkey).await?;
    let mut pool = token_swap_v2::decode_pool(&pool_pubkey, &account_data)?;
    println!("-> Décodage initial réussi. Courbe type: {}.", pool.curve_type);

    println!("[1/2] Hydratation...");
    token_swap_v2::hydrate(&mut pool, rpc_client).await?;
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
        amount_in_sol
    )?;

    Ok(())
}

async fn test_meteora_amm(rpc_client: &RpcClient) -> Result<()> {
    const POOL_ADDRESS: &str = "7rQd8FhC1rimV3v9edCRZ6RNFsJN1puXM9UmjaURJRNj"; // Constant Product (WSOL-NOBODY)
    println!("\n--- Test Meteora AMM ({}) ---", POOL_ADDRESS);
    let pool_pubkey = Pubkey::from_str(POOL_ADDRESS)?;
    let account_data = rpc_client.get_account_data(&pool_pubkey).await?;

    let mut pool = meteora_amm::decode_pool(&pool_pubkey, &account_data)?;
    println!("-> Décodage initial réussi. Courbe détectée: {:?}", pool.curve_type);

    println!("[1/2] Hydratation...");
    meteora_amm::hydrate(&mut pool, rpc_client).await?;

    let fee_percent = (pool.fees.trade_fee_numerator as f64 * 100.0) / pool.fees.trade_fee_denominator as f64;
    println!("-> Hydratation terminée. Frais de trading: {:.4}%.", fee_percent);

    let amount_in = 1 * 10u64.pow(pool.mint_a_decimals as u32);
    println!("\n[2/2] Calcul du quote pour 1 Token A (ex: WSOL)...");
    print_quote_result(&pool, &pool.mint_a, pool.mint_a_decimals, pool.mint_b_decimals, amount_in)?;

    Ok(())
}

async fn test_damm_v2(rpc_client: &RpcClient) -> Result<()> {
    const POOL_ADDRESS: &str = "BnpqhBcR8jUXZjtZ8GrT1YyXPggfvtUonwHcHvu8LKj9"; // BAG-WSOL
    println!("\n--- Test Meteora DAMM v2 (BAG/WSOL: {}) ---", POOL_ADDRESS);
    let pool_pubkey = Pubkey::from_str(POOL_ADDRESS)?;
    let account_data = rpc_client.get_account_data(&pool_pubkey).await?;

    let mut pool = damm_v2::decode_pool(&pool_pubkey, &account_data)?;
    println!("-> Decodage initial reussi.");

    println!("[1/2] Hydratation...");
    damm_v2::hydrate(&mut pool, rpc_client).await?;
    println!("-> Hydratation terminee. Frais de base: {:.4}%.", pool.fee_as_percent());
    println!("   -> Decimale A (BAG): {}, Decimale B (WSOL): {}", pool.mint_a_decimals, pool.mint_b_decimals);

    let amount_in = 10_000 * 10u64.pow(pool.mint_a_decimals as u32);
    println!("\n[2/2] Calcul du quote pour 10000 BAG...");
    print_quote_result(
        &pool,
        &pool.mint_a,
        pool.mint_a_decimals,
        pool.mint_b_decimals,
        amount_in
    )?;

    Ok(())
}

async fn test_pump_amm(rpc_client: &RpcClient) -> Result<()> {
    const POOL_ADDRESS: &str = "2H87WiHP7gMcBGodhRtzgmVc3MYqEx1xPcGdoVUHNQhj"; // DEPI-SOL
    println!("\n--- Test pump.fun AMM ({}) ---", POOL_ADDRESS);
    let pool_pubkey = Pubkey::from_str(POOL_ADDRESS)?;
    let account_data = rpc_client.get_account_data(&pool_pubkey).await?;

    let mut pool = pump_decoders::amm::decode_pool(&pool_pubkey, &account_data)?;
    println!("-> Décodage initial réussi.");

    println!("[1/2] Hydratation...");
    pump_decoders::amm::hydrate(&mut pool, rpc_client).await?;
    println!("-> Hydratation terminée. Frais totaux: {:.4}%.", pool.fee_as_percent());
    println!("   -> Réserve A ({}): {}", pool.mint_a, pool.reserve_a);
    println!("   -> Réserve B (SOL): {}", pool.reserve_b);

    let amount_in_sol = (0.1 * 1_000_000_000.0) as u64;
    println!("\n[2/2] Calcul du quote pour 0.1 SOL...");
    print_quote_result(
        &pool,
        &pool.mint_b,
        pool.mint_b_decimals,
        pool.mint_a_decimals,
        amount_in_sol
    )?;

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
        // Exécutez tous les tests ici
        if let Err(e) = test_ammv4(&rpc_client).await { println!("!! AMMv4 a échoué: {}", e); }
        if let Err(e) = test_cpmm(&rpc_client).await { println!("!! CPMM a échoué: {}", e); }
        if let Err(e) = test_clmm(&rpc_client).await { println!("!! CLMM a échoué: {}", e); }
        if let Err(e) = test_launchpad(&rpc_client).await { println!("!! Launchpad a échoué: {}", e); }
        if let Err(e) = test_meteora_amm(&rpc_client).await { println!("!! Meteora AMM a échoué: {}", e); }
        if let Err(e) = test_damm_v2(&rpc_client).await { println!("!! Meteora DAMM v2 a echoue: {}", e); }
        if let Err(e) = test_dlmm(&rpc_client).await { println!("!! DLMM a échoué: {}", e); }
        if let Err(e) = test_whirlpool(&rpc_client).await { println!("!! Whirlpool a échoué: {}", e); }
        if let Err(e) = test_orca_amm_v2(&rpc_client).await { println!("!! Orca AMM V2 a échoué: {}", e); }
        if let Err(e) = test_orca_amm_v1(&rpc_client).await { println!("!! Orca AMM V1 a échoué: {}", e); }
        if let Err(e) = test_pump_amm(&rpc_client).await { println!("!! pump.fun AMM a échoué: {}", e); }

    } else {
        println!("Mode: Exécution des tests spécifiques: {:?}", args);
        for test_name in args {
            let result = match test_name.as_str() {
                "ammv4" => test_ammv4(&rpc_client).await,
                "cpmm" => test_cpmm(&rpc_client).await,
                "clmm" => test_clmm(&rpc_client).await,
                "launchpad" => test_launchpad(&rpc_client).await,
                "meteora_amm" => test_meteora_amm(&rpc_client).await,
                "damm_v2" => test_damm_v2(&rpc_client).await,
                "dlmm" => test_dlmm(&rpc_client).await, // <--- APPELLE NOTRE NOUVEL ORCHESTRATEUR
                "whirlpool" => test_whirlpool(&rpc_client).await,
                "orca_amm_v2" => test_orca_amm_v2(&rpc_client).await,
                "orca_amm_v1" => test_orca_amm_v1(&rpc_client).await,
                "pump_amm" => test_pump_amm(&rpc_client).await,
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