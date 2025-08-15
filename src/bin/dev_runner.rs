// DANS : src/bin/dev_runner.rs
// VERSION FINALE ET COMPLÈTE

use anyhow::{anyhow, Result};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::sysvar::clock::Clock;
use std::str::FromStr;
use std::env;
use anyhow::bail;


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
use bincode::deserialize;

use solana_sdk::{
    account::Account,
    instruction::Instruction,
    signature::Keypair,
    signer::Signer,
};
use spl_token::state::Account as SplTokenAccount;
use mev::execution::simulate; // Notre module de simulation !
use solana_program_pack::Pack;
use spl_associated_token_account::get_associated_token_address;



// =================================================================================
// FONCTION UTILITAIRE D'AFFICHAGE (INCHANGÉE LOGIQUEMENT, JUSTE AVEC TIMESTAMP)
// =================================================================================
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

// =================================================================================
// DÉFINITIONS DES TESTS INDIVIDUELS (AVEC CORRECTIONS)
// =================================================================================

async fn get_timestamp(rpc_client: &RpcClient) -> Result<i64> {
    let clock_account = rpc_client.get_account(&solana_sdk::sysvar::clock::ID).await?;

    // --- C'est la syntaxe correcte pour bincode v1.3.3 ---
    let clock: Clock = deserialize(&clock_account.data)?;

    Ok(clock.unix_timestamp)
}





// --- TEST DLMM (CORRIGÉ) ---
async fn test_dlmm(rpc_client: &RpcClient, current_timestamp: i64) -> Result<()> {
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
            pool_address: "SfyiR54tjDB13LZv9WYpZbErXMrbdqMgAiAWuCszBwE", // ANI-WSOL
            input_token_mint: "So11111111111111111111111111111111111111112", // WSOL
            input_amount_ui: 1.0,
            description: "ANI-WSOL (Vente de 1 WSOL)",
        },
        DlmmTestCase {
            pool_address: "SfyiR54tjDB13LZv9WYpZbErXMrbdqMgAiAWuCszBwE", // ANI-WSOL
            input_token_mint: "9mAnyxAq8JQieHT7Lc47PVQbTK7ZVaaog8LwAbFzBAGS", // ANI
            input_amount_ui: 142418.913458495,
            description: "ANI-WSOL (Vente de 3600 ANI)",
        },
    ];

    const PROGRAM_ID: &str = "LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo";
    let program_pubkey = Pubkey::from_str(PROGRAM_ID)?;

    for (i, case) in test_cases.iter().enumerate() {
        println!("\n--- [DLMM Cas {}/{}] Test sur {} ---", i + 1, test_cases.len(), case.description);
        let pool_pubkey = Pubkey::from_str(case.pool_address)?;
        let input_mint_pubkey = Pubkey::from_str(case.input_token_mint)?;

        let account_data = rpc_client.get_account_data(&pool_pubkey).await?;
        let mut pool = dlmm::decode_lb_pair(&pool_pubkey, &account_data, &program_pubkey)?;

        println!("[1/2] Hydratation...");
        dlmm::hydrate(&mut pool, rpc_client, 10).await?;
        println!("-> Hydratation terminée. Trouvé {} BinArray(s).",
                 pool.hydrated_bin_arrays.as_ref().unwrap_or(&Default::default()).len());
        println!("   -> Mint A ({} décimales): {}", pool.mint_a_decimals, pool.mint_a);
        println!("   -> Mint B ({} décimales): {}", pool.mint_b_decimals, pool.mint_b);

        println!("\n[2/2] Calcul pour VENDRE {} UI de {}...", case.input_amount_ui, case.input_token_mint);

        let (input_mint_decimals, output_mint_decimals) = if input_mint_pubkey == pool.mint_a {
            (pool.mint_a_decimals, pool.mint_b_decimals)
        } else if input_mint_pubkey == pool.mint_b {
            (pool.mint_b_decimals, pool.mint_a_decimals)
        } else {
            return Err(anyhow!("Le token d'input {} n'appartient pas au pool {}", case.input_token_mint, case.pool_address));
        };

        let amount_in_base_units = (case.input_amount_ui * 10f64.powi(input_mint_decimals as i32)) as u64;

        print_quote_result(
            &pool,
            &input_mint_pubkey,
            input_mint_decimals,
            output_mint_decimals,
            amount_in_base_units,
            current_timestamp
        )?;
    }
    Ok(())
}

fn create_temp_token_account(
    mint: &Pubkey,
    owner: &Pubkey,
    amount: u64
) -> Account {
    let mut data = vec![0u8; SplTokenAccount::LEN];
    let token_account_state = SplTokenAccount {
        mint: *mint,
        owner: *owner,
        amount,
        state: spl_token::state::AccountState::Initialized,
        ..Default::default()
    };
    SplTokenAccount::pack(token_account_state, &mut data).unwrap();

    Account {
        lamports: 2_039_280, // Rent-exempt amount
        data,
        owner: spl_token::id(),
        executable: false,
        rent_epoch: u64::MAX,
    }
}

fn parse_spl_token_transfer_amount_from_logs(logs: &[String]) -> Option<u64> {
    logs.iter()
        .filter(|log| log.contains("spl-token") && log.contains("Transfer"))
        .filter_map(|log| log.split_whitespace().last()?.parse::<u64>().ok())
        .last()
}


// LA NOUVELLE FONCTION DE TEST
async fn test_ammv4(rpc_client: &RpcClient, current_timestamp: i64) -> Result<()> {
    const POOL_ADDRESS: &str = "58oQChx4yWmvKdwLLZzBi4ChoCc2fqCUWBkwMihLYQo2"; // WSOL-USDC
    const INPUT_MINT_STR: &str = "So11111111111111111111111111111111111111112"; // WSOL
    const INPUT_AMOUNT_UI: f64 = 1.0;

    println!("\n--- Test et Simulation (AVEC DÉBOGAGE) Raydium AMMv4 ({}) ---", POOL_ADDRESS);
    let pool_pubkey = Pubkey::from_str(POOL_ADDRESS)?;

    // --- ÉTAPE 1 : PRÉDICTION LOCALE ---
    println!("[1/3] Hydratation et prédiction locale...");
    let pool_account_data = rpc_client.get_account_data(&pool_pubkey).await?;
    let mut pool = amm_v4::decode_pool(&pool_pubkey, &pool_account_data)?;
    amm_v4::hydrate(&mut pool, rpc_client).await?;

    let input_mint_pubkey = Pubkey::from_str(INPUT_MINT_STR)?;
    let (input_decimals, output_decimals, output_mint_pubkey) = if input_mint_pubkey == pool.mint_a {
        (pool.mint_a_decimals, pool.mint_b_decimals, pool.mint_b)
    } else {
        (pool.mint_b_decimals, pool.mint_a_decimals, pool.mint_a)
    };
    let amount_in_base_units = (INPUT_AMOUNT_UI * 10f64.powi(input_decimals as i32)) as u64;

    let predicted_amount_out = pool.get_quote(&input_mint_pubkey, amount_in_base_units, current_timestamp)?;
    let ui_predicted_amount_out = predicted_amount_out as f64 / 10f64.powi(output_decimals as i32);
    println!("-> PRÉDICTION LOCALE: {}", ui_predicted_amount_out);

    // --- ÉTAPE 2 : PRÉPARATION DE LA SIMULATION ---
    println!("\n[2/3] Préparation de la simulation...");
    let payer = Keypair::new();
    let user_source_ata = get_associated_token_address(&payer.pubkey(), &input_mint_pubkey);
    let user_destination_ata = get_associated_token_address(&payer.pubkey(), &output_mint_pubkey);

    let swap_ix = amm_v4::create_swap_instruction(
        &pool, &user_source_ata, &user_destination_ata, &payer.pubkey(), amount_in_base_units, 0,
    )?;

    let mut keys_for_sim: Vec<Pubkey> = swap_ix.accounts.iter().map(|meta| meta.pubkey).collect();
    keys_for_sim.retain(|&key| key != payer.pubkey() && key != user_source_ata && key != user_destination_ata);
    keys_for_sim.sort();
    keys_for_sim.dedup();

    println!("-> Récupération de l'état on-chain de {} comptes...", keys_for_sim.len());

    let account_data_for_sim = rpc_client.get_multiple_accounts(&keys_for_sim).await?;

    // --- LE DÉBOGAGE EST ICI ---
    let mut missing_accounts = false;
    for (i, key) in keys_for_sim.iter().enumerate() {
        if account_data_for_sim[i].is_none() {
            println!("!!!!!! COMPTE MANQUANT !!!!!! L'appel RPC pour {} a retourné None.", key);
            missing_accounts = true;
        }
    }
    if missing_accounts {
        bail!("Un ou plusieurs comptes n'ont pas pu être récupérés depuis le RPC. Arrêt.");
    }
    // --- FIN DU DÉBOGAGE ---

    let mut accounts_to_load: Vec<(Pubkey, Account)> = keys_for_sim
        .into_iter()
        .zip(account_data_for_sim.into_iter())
        .filter_map(|(pubkey, acc_opt)| acc_opt.map(|acc| (pubkey, acc)))
        .collect();

    let user_source_account = create_temp_token_account(&input_mint_pubkey, &payer.pubkey(), amount_in_base_units);
    let user_dest_account = create_temp_token_account(&output_mint_pubkey, &payer.pubkey(), 0);
    accounts_to_load.push((user_source_ata, user_source_account));
    accounts_to_load.push((user_destination_ata, user_dest_account));

    println!("-> {} comptes chargés pour le contexte de simulation.", accounts_to_load.len());

    // --- ÉTAPE 3 : SIMULATION ET ANALYSE ---
    println!("\n[3/3] Exécution de la simulation...");
    let sim_result = simulate::simulate_instruction(rpc_client, swap_ix, &payer, accounts_to_load).await?;
    println!("-> Simulation réussie !");

    let simulated_amount_out = match sim_result.logs {
        Some(logs) => parse_spl_token_transfer_amount_from_logs(&logs)
            .ok_or_else(|| anyhow!("Impossible de trouver le transfert SPL dans les logs"))?,
        None => bail!("Les logs de la simulation sont vides."),
    };
    let ui_simulated_amount_out = simulated_amount_out as f64 / 10f64.powi(output_decimals as i32);

    println!("\n--- COMPARAISON ---");
    println!("Montant PRÉDIT (local) : {}", ui_predicted_amount_out);
    println!("Montant SIMULÉ (on-chain) : {}", ui_simulated_amount_out);
    let difference = (ui_predicted_amount_out - ui_simulated_amount_out).abs();
    let difference_percent = if ui_simulated_amount_out > 0.0 { (difference / ui_simulated_amount_out) * 100.0 } else { 0.0 };
    println!("-> Différence relative: {:.4} %", difference_percent);

    if difference_percent < 0.01 {
        println!("✅ SUCCÈS ! Le décodeur est parfaitement précis.");
    } else {
        println!("⚠️ ÉCHEC ! La différence est trop grande.");
    }

    Ok(())
}

async fn test_cpmm(rpc_client: &RpcClient, current_timestamp: i64) -> Result<()> {
    // --- CONSTANTES POUR LE TEST DE LA TRANSACTION ---
    const POOL_ADDRESS: &str = "Q2sPHPdUWFMg7M7wwrQKLrn619cAucfRsmhVJffodSp"; // USELESS-WSOL
    const INPUT_MINT: &str = "Dz9mQ9NzkBcCsuGPFJ3r1bS4wgqKMHBPiVuniW8Mbonk"; // WSOL
    const INPUT_AMOUNT_UI: f64 = 4236.0; // Montant de WSOL de l'image
    // --- FIN DES CONSTANTES ---

    println!("\n--- Test Raydium CPMM ({}) ---", POOL_ADDRESS);
    let pool_pubkey = Pubkey::from_str(POOL_ADDRESS)?;
    let account_data = rpc_client.get_account_data(&pool_pubkey).await?;
    let mut pool = cpmm::decode_pool(&pool_pubkey, &account_data)?;

    println!("[1/2] Hydratation...");
    cpmm::hydrate(&mut pool, rpc_client).await?;
    println!("-> Hydratation terminée. Frais: {:.4}%.", pool.fee_as_percent());

    let input_mint_pubkey = Pubkey::from_str(INPUT_MINT)?;

    // --- DÉTERMINATION DYNAMIQUE DES DÉCIMALES ---
    let (input_decimals, output_decimals) = if input_mint_pubkey == pool.token_0_mint {
        (pool.mint_0_decimals, pool.mint_1_decimals)
    } else {
        (pool.mint_1_decimals, pool.mint_0_decimals)
    };

    println!("   -> Token d'entrée (WSOL): {} ({} décimales détectées)", input_mint_pubkey, input_decimals);
    println!("   -> Token de sortie (USELESS): {} ({} décimales détectées)", if input_mint_pubkey == pool.token_0_mint { pool.token_1_mint } else { pool.token_0_mint }, output_decimals);
    // --- FIN DE LA DÉTERMINATION ---

    let amount_in_base_units = (INPUT_AMOUNT_UI * 10f64.powi(input_decimals as i32)) as u64;
    println!("\n[2/2] Calcul du quote pour VENDRE {} UI de WSOL...", INPUT_AMOUNT_UI);
    print_quote_result(&pool, &input_mint_pubkey, input_decimals, output_decimals, amount_in_base_units, current_timestamp)?;

    Ok(())
}

// DANS : src/bin/dev_runner.rs

async fn test_clmm(rpc_client: &RpcClient, current_timestamp: i64) -> Result<()> {
    // --- TEST BASÉ SUR LA TRANSACTION DE L'IMAGE : Vente de WSOL pour du LAUNCHCOIN ---
    const POOL_ADDRESS: &str = "YrrUStgPugDp8BbfosqDeFssen6sA75ZS1QJvgnHtmY"; // Pool WSOL-LAUNCHCOIN
    const PROGRAM_ID: &str = "CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK";

    // On vend du WSOL, comme dans la première transaction de l'image
    const INPUT_MINT: &str = "So11111111111111111111111111111111111111112"; //Ey59PH7Z4BFU4HjyKnyMdWt5GGN76KazTAwQihoUXRnk
    const INPUT_AMOUNT_UI: f64 = 0.523441514;
    // --- FIN DE LA CONFIGURATION DU TEST ---

    println!("\n--- Test Raydium CLMM ({}) ---", POOL_ADDRESS);
    let pool_pubkey = Pubkey::from_str(POOL_ADDRESS)?;
    let program_pubkey = Pubkey::from_str(PROGRAM_ID)?;
    let account_data = rpc_client.get_account_data(&pool_pubkey).await?;
    let mut pool = clmm_pool::decode_pool(&pool_pubkey, &account_data, &program_pubkey)?;

    println!("[1/2] Hydratation...");
    clmm_pool::hydrate(&mut pool, rpc_client).await?;
    let tick_arrays_found = pool.tick_arrays.as_ref().map_or(0, |arrays| arrays.len());
    println!("-> Hydratation terminée. Frais: {:.4}%. Trouvé {} TickArray(s).", pool.fee_as_percent(), tick_arrays_found);

    let input_mint_pubkey = Pubkey::from_str(INPUT_MINT)?;

    // --- Logique de détermination robuste ---
    let (input_decimals, output_decimals, output_mint_pubkey) = if input_mint_pubkey == pool.mint_a {
        (pool.mint_a_decimals, pool.mint_b_decimals, pool.mint_b)
    } else if input_mint_pubkey == pool.mint_b {
        (pool.mint_b_decimals, pool.mint_a_decimals, pool.mint_a)
    } else {
        bail!("ERREUR FATALE: Le token d'input {} n'appartient PAS au pool {}. Mints du pool: {}, {}", INPUT_MINT, POOL_ADDRESS, pool.mint_a, pool.mint_b);
    };

    println!("   -> Token d'entrée : {} ({} décimales détectées)", input_mint_pubkey, input_decimals);
    println!("   -> Token de sortie: {} ({} décimales détectées)", output_mint_pubkey, output_decimals);

    let amount_in_base_units = (INPUT_AMOUNT_UI * 10f64.powi(input_decimals as i32)) as u64;
    println!("\n[2/2] Calcul du quote pour VENDRE {} UI de {}...", INPUT_AMOUNT_UI, INPUT_MINT);

    print_quote_result(
        &pool,
        &input_mint_pubkey,
        input_decimals,
        output_decimals,
        amount_in_base_units,
        current_timestamp
    )?;

    Ok(())
}

async fn test_launchpad(rpc_client: &RpcClient, current_timestamp: i64) -> Result<()> {
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
    print_quote_result(&pool, &pool.mint_b, 6, 6, small_amount_in, current_timestamp)?;

    Ok(())
}

async fn test_whirlpool(rpc_client: &RpcClient, current_timestamp: i64) -> Result<()> {
    const POOL_ADDRESS: &str = "44W73kGYQgXCTNkGxUmHv8DDBPCxojBcX49uuKmbFc9U"; // WSOL-USDC
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
        &pool.mint_a,
        pool.mint_a_decimals,
        pool.mint_b_decimals,
        sol_amount_in,
        current_timestamp
    )?;

    Ok(())
}

async fn test_orca_amm_v1(rpc_client: &RpcClient, current_timestamp: i64) -> Result<()> {
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
        amount_in,
        current_timestamp
    )?;

    Ok(())
}

async fn test_orca_amm_v2(rpc_client: &RpcClient, current_timestamp: i64) -> Result<()> {
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
        amount_in_sol,
        current_timestamp
    )?;

    Ok(())
}

async fn test_meteora_amm(rpc_client: &RpcClient, current_timestamp: i64) -> Result<()> {
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
    print_quote_result(&pool, &pool.mint_a, pool.mint_a_decimals, pool.mint_b_decimals, amount_in, current_timestamp)?;

    Ok(())
}

async fn test_damm_v2(rpc_client: &RpcClient, current_timestamp: i64) -> Result<()> {
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

    // --- DÉBUT DE LA MODIFICATION DU TEST ---

    // Nous simulons maintenant la vente de 0.999 WSOL, comme dans la transaction on-chain.
    // WSOL est mint_b et a 9 décimales.
    let amount_in_ui = 0.999;
    let amount_in_base_units = (amount_in_ui * 10f64.powi(pool.mint_b_decimals as i32)) as u64;

    println!("\n[2/2] Calcul du quote pour VENDRE {} WSOL...", amount_in_ui);
    print_quote_result(
        &pool,
        &pool.mint_b, // On entre du WSOL (mint_b)
        pool.mint_b_decimals, // Décimales de l'input (WSOL)
        pool.mint_a_decimals, // Décimales de l'output (BAG)
        amount_in_base_units,
        current_timestamp
    )?;

    // --- FIN DE LA MODIFICATION DU TEST ---

    Ok(())
}

async fn test_pump_amm(rpc_client: &RpcClient, current_timestamp: i64) -> Result<()> {
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
        amount_in_sol,
        current_timestamp
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
    let current_timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs() as i64;
    println!("-> Timestamp du cluster utilisé pour tous les tests: {}", current_timestamp);

    let args: Vec<String> = env::args().skip(1).collect();

    // On récupère le timestamp une seule fois pour tous les tests
    let current_timestamp = get_timestamp(&rpc_client).await?;
    println!("-> Timestamp du cluster utilisé pour tous les tests: {}", current_timestamp);


    if args.is_empty() {
        println!("Mode: Exécution de tous les tests.");
        if let Err(e) = test_ammv4(&rpc_client, current_timestamp).await { println!("!! AMMv4 a échoué: {}", e); }
        if let Err(e) = test_cpmm(&rpc_client, current_timestamp).await { println!("!! CPMM a échoué: {}", e); }
        if let Err(e) = test_clmm(&rpc_client, current_timestamp).await { println!("!! CLMM a échoué: {}", e); }
        if let Err(e) = test_launchpad(&rpc_client, current_timestamp).await { println!("!! Launchpad a échoué: {}", e); }
        if let Err(e) = test_meteora_amm(&rpc_client, current_timestamp).await { println!("!! Meteora AMM a échoué: {}", e); }
        if let Err(e) = test_damm_v2(&rpc_client, current_timestamp).await { println!("!! Meteora DAMM v2 a echoue: {}", e); }
        if let Err(e) = test_dlmm(&rpc_client, current_timestamp).await { println!("!! DLMM a échoué: {}", e); }
        if let Err(e) = test_whirlpool(&rpc_client, current_timestamp).await { println!("!! Whirlpool a échoué: {}", e); }
        if let Err(e) = test_orca_amm_v2(&rpc_client, current_timestamp).await { println!("!! Orca AMM V2 a échoué: {}", e); }
        if let Err(e) = test_orca_amm_v1(&rpc_client, current_timestamp).await { println!("!! Orca AMM V1 a échoué: {}", e); }
        if let Err(e) = test_pump_amm(&rpc_client, current_timestamp).await { println!("!! pump.fun AMM a échoué: {}", e); }
    } else {
        println!("Mode: Exécution des tests spécifiques: {:?}", args);
        for test_name in args {
            let result = match test_name.as_str() {
                "ammv4" => test_ammv4(&rpc_client, current_timestamp).await,
                "cpmm" => test_cpmm(&rpc_client, current_timestamp).await,
                "clmm" => test_clmm(&rpc_client, current_timestamp).await,
                "launchpad" => test_launchpad(&rpc_client, current_timestamp).await,
                "meteora_amm" => test_meteora_amm(&rpc_client, current_timestamp).await,
                "damm_v2" => test_damm_v2(&rpc_client, current_timestamp).await,
                "dlmm" => test_dlmm(&rpc_client, current_timestamp).await,
                "whirlpool" => test_whirlpool(&rpc_client, current_timestamp).await,
                "orca_amm_v2" => test_orca_amm_v2(&rpc_client, current_timestamp).await,
                "orca_amm_v1" => test_orca_amm_v1(&rpc_client, current_timestamp).await,
                "pump_amm" => test_pump_amm(&rpc_client, current_timestamp).await,
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