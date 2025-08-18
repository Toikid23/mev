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
use solana_sdk::transaction::Transaction;
use std::collections::HashSet;
use std::collections::HashMap;
use anchor_lang::{AnchorDeserialize, Discriminator};
use borsh::BorshDeserialize;
use solana_client::rpc_config::RpcSimulateTransactionConfig; // Ajoutez cet import en haut
use solana_account_decoder::UiAccountData;
use num_integer::Integer; // Pour la division au plafond
use solana_sdk::instruction::AccountMeta;
use spl_associated_token_account::get_associated_token_address_with_program_id;
use solana_transaction_status::UiTransactionEncoding;





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

fn parse_spl_token_transfer_amount_from_logs(logs: &[String]) -> Option<u64> {
    // On cherche d'abord le format le plus récent (ex: "Program log: Instruction: TransferChecked")
    for log in logs.iter().rev() { // On itère depuis la fin, le transfert de sortie est souvent le dernier.
        if log.contains("Program log: Instruction: TransferChecked") {
            // Dans ce format, le montant n'est pas dans le log, il faut regarder les logs suivants du programme token.
            // C'est complexe. Utilisons une méthode plus simple.
        }
    }

    // MÉTHODE UNIVERSELLE : Le programme principal émet souvent un log juste avant le transfert.
    // Le log de transfert réel du programme token ressemble à ça :
    // "Program Tokenkeg... success"
    // "Program log: Transfer XXX tokens"
    // Malheureusement, la simulation ne retourne pas toujours ces logs détaillés.

    // LA MÉTHODE LA PLUS SIMPLE ET QUI MARCHE POUR CPMM :
    // Raydium log un événement. Puisque le parsing de cet événement est capricieux,
    // extrayons le montant manuellement.
    for log in logs {
        if let Some(data_str) = log.strip_prefix("Program data: ") {
            if let Ok(bytes) = base64::decode(data_str) {
                // La structure de l'événement est :
                // pub pool_id: Pubkey,            // 32 bytes
                // pub input_vault_before: u64,    // 8 bytes
                // pub output_vault_before: u64,   // 8 bytes
                // pub input_amount: u64,          // 8 bytes
                // pub output_amount: u64,         // 8 bytes <--- CE QU'ON VEUT

                // Offset de output_amount = 32 + 8 + 8 + 8 = 56
                const OFFSET: usize = 56;
                if bytes.len() >= OFFSET + 8 {
                    let amount_bytes: [u8; 8] = bytes[OFFSET..OFFSET + 8].try_into().ok()?;
                    return Some(u64::from_le_bytes(amount_bytes));
                }
            }
        }
    }


    // Si la méthode ci-dessus échoue, on retombe sur l'ancienne méthode pour la compatibilité
    logs.iter()
        .find(|log| log.contains("spl_token") && log.contains("Transfer") && log.starts_with("Program log:"))
        .and_then(|log| log.split_whitespace().last()?.parse::<u64>().ok())
}


#[derive(BorshDeserialize, Debug)]
pub struct CpmmSwapEvent {
    pub pool_id: Pubkey,
    pub input_vault_before: u64,
    pub output_vault_before: u64,
    pub input_amount: u64,
    pub output_amount: u64,
    pub input_transfer_fee: u64,
    pub output_transfer_fee: u64,
    pub base_input: bool,
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
        lamports: 2_039_280, // Montant pour être rent-exempt
        data,
        owner: spl_token::id(),
        executable: false,
        rent_epoch: u64::MAX,
    }
}


fn parse_swap_event_from_logs(logs: &[String], is_base_input: bool) -> Option<u64> {
    // Discriminator pour SwapEvent, tiré de l'IDL de Raydium
    const SWAP_EVENT_DISCRIMINATOR: [u8; 8] = [64, 198, 205, 232, 38, 8, 113, 226];

    for log in logs {
        if let Some(data_str) = log.strip_prefix("Program data: ") {
            if let Ok(bytes) = base64::decode(data_str) {
                if bytes.len() > 8 && bytes.starts_with(&SWAP_EVENT_DISCRIMINATOR) {
                    let mut event_data: &[u8] = &bytes[8..];
                    if let Ok(event) = SwapEvent::try_from_slice(&mut event_data) {
                        if is_base_input {
                            return Some(event.amount_1);
                        } else {
                            return Some(event.amount_0);
                        }
                    }
                }
            }
        }
    }
    None
}

#[derive(BorshDeserialize, Debug)]
pub struct SwapEvent {
    // discriminator: [u8; 8], // Le discriminator est déjà vérifié, on ne le met pas dans la struct
    pub pool_state: Pubkey,
    pub sender: Pubkey,
    pub token_account_0: Pubkey,
    pub token_account_1: Pubkey,
    pub amount_0: u64,
    pub transfer_fee_0: u64,
    pub amount_1: u64,
    pub transfer_fee_1: u64,
    pub zero_for_one: bool,
    pub sqrt_price_x64: u128,
    pub liquidity: u128,
    pub tick: i32,
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

    let payer_account = Account {
        lamports: 1_000_000_000,
        data: vec![],
        owner: solana_sdk::system_program::id(),
        executable: false,
        rent_epoch: u64::MAX,
    };
    accounts_to_load.push((payer.pubkey(), payer_account));


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



fn parse_output_amount_from_cpmm_event(logs: &[String]) -> Option<u64> {
    for log in logs {
        if let Some(data_str) = log.strip_prefix("Program data: ") {
            if let Ok(bytes) = base64::decode(data_str) {
                // Si l'événement est préfixé par un discriminateur de 8 octets
                const DISCRIMINATOR_SIZE: usize = 8;
                // Offset de `output_amount` après le discriminateur : 56 + 8 = 64
                const OUTPUT_AMOUNT_OFFSET: usize = 64;

                if bytes.len() >= OUTPUT_AMOUNT_OFFSET + 8 {
                    let amount_slice = &bytes[OUTPUT_AMOUNT_OFFSET..OUTPUT_AMOUNT_OFFSET + 8];
                    if let Ok(amount_bytes) = amount_slice.try_into() {
                        return Some(u64::from_le_bytes(amount_bytes));
                    }
                }
            }
        }
    }
    None
}

// --- LA FONCTION DE TEST FINALE ---
async fn test_cpmm_with_simulation(rpc_client: &RpcClient, payer_keypair: &Keypair, current_timestamp: i64) -> Result<()> {
    // LA BONNE ADRESSE DE POOL
    const POOL_ADDRESS: &str = "8ujpQXxnnWvRohU2oCe3eaSzoL7paU2uj3fEn4Zp72US";
    const INPUT_MINT_STR: &str = "So11111111111111111111111111111111111111112";
    const INPUT_AMOUNT_UI: f64 = 0.05;

    println!("\n--- Test et Simulation (Final) Raydium CPMM ({}) ---", POOL_ADDRESS);
    let pool_pubkey = Pubkey::from_str(POOL_ADDRESS)?;

    println!("[1/3] Hydratation et prédiction locale...");
    let pool_account_data = rpc_client.get_account_data(&pool_pubkey).await?;
    let mut pool = cpmm::decode_pool(&pool_pubkey, &pool_account_data)?;
    cpmm::hydrate(&mut pool, rpc_client).await?;

    let input_mint_pubkey = Pubkey::from_str(INPUT_MINT_STR)?;
    let (input_decimals, output_decimals, output_mint_pubkey, input_is_token_0) = if input_mint_pubkey == pool.token_0_mint {
        (pool.mint_0_decimals, pool.mint_1_decimals, pool.token_1_mint, true)
    } else {
        (pool.mint_1_decimals, pool.mint_0_decimals, pool.token_0_mint, false)
    };

    let amount_in_base_units = (INPUT_AMOUNT_UI * 10f64.powi(input_decimals as i32)) as u64;
    let predicted_amount_out = pool.get_quote(&input_mint_pubkey, amount_in_base_units, current_timestamp)?;
    let ui_predicted_amount_out = predicted_amount_out as f64 / 10f64.powi(output_decimals as i32);
    println!("-> PRÉDICTION LOCALE: {} UI", ui_predicted_amount_out);

    println!("\n[2/3] Préparation de la transaction...");
    let payer_pubkey = payer_keypair.pubkey();
    let user_source_ata = get_associated_token_address(&payer_pubkey, &input_mint_pubkey);
    let user_destination_ata = get_associated_token_address(&payer_pubkey, &output_mint_pubkey);

    let mut instructions = Vec::new();
    if rpc_client.get_account(&user_destination_ata).await.is_err() {
        instructions.push(
            spl_associated_token_account::instruction::create_associated_token_account(
                &payer_pubkey, &payer_pubkey, &output_mint_pubkey, &pool.token_1_program,
            ),
        );
    }

    let swap_ix = pool.create_swap_instruction(
        &user_source_ata, &user_destination_ata, &payer_pubkey,
        input_is_token_0, amount_in_base_units, 0,
    )?;
    instructions.push(swap_ix);

    let recent_blockhash = rpc_client.get_latest_blockhash().await?;
    let transaction = Transaction::new_signed_with_payer(
        &instructions, Some(&payer_pubkey), &[payer_keypair], recent_blockhash,
    );

    println!("\n[3/3] Exécution de la simulation standard...");
    let sim_result = rpc_client.simulate_transaction(&transaction).await?.value;

    if let Some(err) = sim_result.err { bail!("La simulation a échoué: {:?}", err); }

    let simulated_amount_out = sim_result.logs
        .as_ref()
        .and_then(|logs| parse_output_amount_from_cpmm_event(logs))
        .ok_or_else(|| {
            if let Some(logs) = &sim_result.logs {
                println!("\n--- LOGS COMPLETS (Montant non trouvé) ---");
                for log in logs {
                    println!("{}", log);
                }
                println!("-------------------------------------------");
            }
            anyhow!("Impossible de parser l'événement de swap depuis les logs")
        })?;

    let ui_simulated_amount_out = simulated_amount_out as f64 / 10f64.powi(output_decimals as i32);

    println!("\n--- COMPARAISON (CPMM) ---");
    println!("Montant PRÉDIT (local)    : {}", ui_predicted_amount_out);
    println!("Montant SIMULÉ (on-chain) : {}", ui_simulated_amount_out);
    let difference = (ui_predicted_amount_out - ui_simulated_amount_out).abs();
    let difference_percent = if ui_simulated_amount_out > 0.0 { (difference / ui_simulated_amount_out) * 100.0 } else { 0.0 };
    println!("-> Différence relative: {:.6} %", difference_percent);

    if difference_percent < 0.01 {
        println!("✅ SUCCÈS ! Le décodeur CPMM est parfaitement précis.");
    } else {
        println!("⚠️ ÉCHEC ! La différence est trop grande.");
    }

    Ok(())
}


async fn test_clmm(rpc_client: &RpcClient, payer: &Keypair, current_timestamp: i64) -> Result<()> {
    const POOL_ADDRESS: &str = "YrrUStgPugDp8BbfosqDeFssen6sA75ZS1QJvgnHtmY";
    const PROGRAM_ID: &str = "CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK";
    const INPUT_MINT_STR: &str = "So11111111111111111111111111111111111111112";
    const INPUT_AMOUNT_UI: f64 = 0.1;

    println!("\n--- Test et Simulation (VRAI COMPTE) Raydium CLMM ({}) ---", POOL_ADDRESS);
    let pool_pubkey = Pubkey::from_str(POOL_ADDRESS)?;
    let program_pubkey = Pubkey::from_str(PROGRAM_ID)?;

    println!("[1/3] Hydratation et prédiction locale...");
    let pool_account_data = rpc_client.get_account_data(&pool_pubkey).await?;
    let mut pool = clmm_pool::decode_pool(&pool_pubkey, &pool_account_data, &program_pubkey)?;
    clmm_pool::hydrate(&mut pool, rpc_client).await?;
    let input_mint_pubkey = Pubkey::from_str(INPUT_MINT_STR)?;
    let (input_decimals, output_decimals, output_mint_pubkey) = if input_mint_pubkey == pool.mint_a {
        (pool.mint_a_decimals, pool.mint_b_decimals, pool.mint_b)
    } else { (pool.mint_b_decimals, pool.mint_a_decimals, pool.mint_a) };
    let amount_in_base_units = (INPUT_AMOUNT_UI * 10f64.powi(input_decimals as i32)) as u64;

    let (predicted_amount_out, _) = pool.get_quote_with_tick_arrays(&input_mint_pubkey, amount_in_base_units, current_timestamp)?;
    let ui_predicted_amount_out = predicted_amount_out as f64 / 10f64.powi(output_decimals as i32);
    println!("-> PRÉDICTION LOCALE (potentiellement fausse): {}", ui_predicted_amount_out);

    println!("\n[2/3] Préparation de la simulation...");

    // --- LA LOGIQUE DE TICK_ARRAY CORRECTE ET DYNAMIQUE (selon les logs d'erreur) ---
    let is_base_input = input_mint_pubkey == pool.mint_a;
    let ticks_in_array = (mev::decoders::raydium_decoders::tick_array::TICK_ARRAY_SIZE as i32) * (pool.tick_spacing as i32);

    // 1. Obtenir tous les index des TickArrays initialisés que nous avons hydratés.
    let mut initialized_indices: Vec<i32> = pool.tick_arrays.as_ref().unwrap().keys().cloned().collect();
    initialized_indices.sort(); // S'assurer qu'ils sont triés

    // 2. Trouver le premier TickArray initialisé dans la direction du swap, COMME LE DEMANDE LE LOG D'ERREUR.
    let first_array_start_index = if is_base_input {
        // Le prix baisse, on cherche le plus grand index <= au tick actuel.
        *initialized_indices.iter().rev().find(|&&i| i <= pool.tick_current).unwrap_or(&pool.tick_current)
    } else {
        // Le prix monte, on cherche le plus petit index >= au tick actuel.
        *initialized_indices.iter().find(|&&i| i >= pool.tick_current).unwrap_or(&pool.tick_current)
    };

    // 3. Construire la liste des 3 TickArrays attendus à partir de ce point de départ.
    let final_tick_arrays: Vec<Pubkey> = if is_base_input {
        vec![
            first_array_start_index,
            first_array_start_index - ticks_in_array,
            first_array_start_index - (2 * ticks_in_array),
        ]
    } else {
        vec![
            first_array_start_index,
            first_array_start_index + ticks_in_array,
            first_array_start_index + (2 * ticks_in_array),
        ]
    }.into_iter().map(|index| {
        mev::decoders::raydium_decoders::tick_array::get_tick_array_address(&pool.address, index, &pool.program_id)
    }).collect();
    // --- FIN DE LA LOGIQUE CORRECTE ---

    let user_source_ata = get_associated_token_address(&payer.pubkey(), &input_mint_pubkey);
    let user_destination_ata = spl_associated_token_account::get_associated_token_address_with_program_id(&payer.pubkey(), &output_mint_pubkey, &spl_token_2022::id());

    let swap_ix = pool.create_swap_instruction(
        &input_mint_pubkey, &user_source_ata, &user_destination_ata, &payer.pubkey(),
        amount_in_base_units, 0, final_tick_arrays,
    )?;

    let mut transaction = Transaction::new_with_payer(&[swap_ix], Some(&payer.pubkey()));
    transaction.sign(&[payer], rpc_client.get_latest_blockhash().await?);

    println!("\n[3/3] Exécution de la simulation standard...");
    let sim_result = rpc_client.simulate_transaction(&transaction).await?.value;

    if sim_result.err.is_some() {
        println!("LOGS DE SIMULATION DÉTAILLÉS:\n{:#?}", sim_result.logs);
        bail!("La simulation a échoué. Cause : {:?}", sim_result.err);
    }

    println!("✅✅✅ VICTOIRE ! La simulation a réussi !");
    println!("Maintenant, vérifions le résultat...");

    let simulated_amount_out = match sim_result.logs {
        Some(logs) => parse_swap_event_from_logs(&logs, is_base_input).ok_or_else(|| {
            anyhow!("Impossible de trouver le transfert dans les logs de simulation.")
        })?,
        None => bail!("Logs de simulation vides."),
    };
    let ui_simulated_amount_out = simulated_amount_out as f64 / 10f64.powi(output_decimals as i32);

    println!("\n--- COMPARAISON (CLMM) ---");
    println!("Montant PRÉDIT (local)  : {}", ui_predicted_amount_out);
    println!("Montant SIMULÉ (on-chain) : {}", ui_simulated_amount_out);
    let difference = (ui_predicted_amount_out - ui_simulated_amount_out).abs();
    let difference_percent = if ui_simulated_amount_out > 0.0 { (difference / ui_simulated_amount_out) * 100.0 } else { 0.0 };
    println!("-> Différence relative: {:.4} %", difference_percent);

    if difference_percent < 0.1 {
        println!("✅ La prédiction de `get_quote` est correcte !");
    } else {
        println!("⚠️ La simulation fonctionne, mais la prédiction de `get_quote` est à revoir.");
    }

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

#[derive(BorshDeserialize, Debug)]
pub struct TradedEvent {
    pub whirlpool: Pubkey,
    pub a_to_b: bool,
    pub pre_sqrt_price: u128,
    pub post_sqrt_price: u128,
    pub input_amount: u64,
    pub output_amount: u64,
    pub input_transfer_fee: u64,
    pub output_transfer_fee: u64,
    pub lp_fee: u64,
    pub protocol_fee: u64,
}

// Ajoutez cette fonction de parsing avec les autres.
fn parse_traded_event_from_logs(logs: &Vec<String>) -> Option<u64> {
    // *** LA CORRECTION EST ICI ***
    // Le VRAI discriminateur, décodé depuis les logs que vous avez fournis.
    const TRADED_EVENT_DISCRIMINATOR: [u8; 8] = [220, 175, 38, 190, 76, 174, 130, 107];

    for log in logs {
        if let Some(data_str) = log.strip_prefix("Program data: ") {
            if let Ok(bytes) = base64::decode(data_str) {
                if bytes.len() > 8 && bytes.starts_with(&TRADED_EVENT_DISCRIMINATOR) {
                    let mut event_data: &[u8] = &bytes[8..];
                    if let Ok(event) = TradedEvent::try_from_slice(&mut event_data) {
                        return Some(event.output_amount);
                    }
                }
            }
        }
    }
    None
}


// dans src/bin/dev_runner.rs


async fn test_whirlpool_with_simulation(rpc_client: &RpcClient, payer: &Keypair, _current_timestamp: i64) -> Result<()> {
    const POOL_ADDRESS: &str = "Czfq3xZZDmsdGdUyrNLtRhGc47cXcZtLG4crryfu44zE";
    const INPUT_MINT_STR: &str = "So11111111111111111111111111111111111111112";
    const INPUT_AMOUNT_UI: f64 = 0.05;

    println!("\n--- Test et Simulation Orca Whirlpool ({}) ---", POOL_ADDRESS);
    let pool_pubkey = Pubkey::from_str(POOL_ADDRESS)?;
    let input_mint_pubkey = Pubkey::from_str(INPUT_MINT_STR)?;

    // --- 1. PRÉDICTION LOCALE ---
    println!("[1/3] Hydratation et prédiction locale...");
    let pool_account_data = rpc_client.get_account_data(&pool_pubkey).await?;
    let mut pool = whirlpool_decoder::decode_pool(&pool_pubkey, &pool_account_data)?;
    whirlpool_decoder::hydrate(&mut pool, rpc_client).await?;

    let (input_decimals, output_decimals, output_mint_pubkey) = if input_mint_pubkey == pool.mint_a {
        (pool.mint_a_decimals, pool.mint_b_decimals, pool.mint_b)
    } else if input_mint_pubkey == pool.mint_b {
        (pool.mint_b_decimals, pool.mint_a_decimals, pool.mint_a)
    } else {
        bail!("Le token d'input {} n'appartient pas au pool {}", input_mint_pubkey, pool_pubkey);
    };
    let amount_in_base_units = (INPUT_AMOUNT_UI * 10f64.powi(input_decimals as i32)) as u64;
    
    let predicted_amount_out = pool.get_quote_with_rpc(&input_mint_pubkey, amount_in_base_units, rpc_client).await?;

    let ui_predicted_amount_out = predicted_amount_out as f64 / 10f64.powi(output_decimals as i32);
    println!("-> PRÉDICTION LOCALE: {} UI", ui_predicted_amount_out);

    // --- Le reste de la fonction est identique à la version qui fonctionnait ---
    // (Je le remets ici pour être complet)
    // --- 2. PRÉPARATION DE LA SIMULATION ---
    println!("\n[2/3] Préparation de la simulation (en supposant que les ATAs existent)...");
    let user_ata_for_token_a = get_associated_token_address(&payer.pubkey(), &pool.mint_a);
    let user_ata_for_token_b = get_associated_token_address(&payer.pubkey(), &pool.mint_b);
    let a_to_b = input_mint_pubkey == pool.mint_a;
    let (user_source_ata, user_destination_ata) = if a_to_b { (user_ata_for_token_a, user_ata_for_token_b) } else { (user_ata_for_token_b, user_ata_for_token_a) };
    let ticks_in_array = (mev::decoders::orca_decoders::tick_array::TICK_ARRAY_SIZE as i32) * (pool.tick_spacing as i32);
    let current_array_start_index = mev::decoders::orca_decoders::tick_array::get_start_tick_index(pool.tick_current_index, pool.tick_spacing);
    let tick_array_addresses: [Pubkey; 3] = if a_to_b {
        [
            mev::decoders::orca_decoders::tick_array::get_tick_array_address(&pool.address, current_array_start_index, &pool.program_id),
            mev::decoders::orca_decoders::tick_array::get_tick_array_address(&pool.address, current_array_start_index - ticks_in_array, &pool.program_id),
            mev::decoders::orca_decoders::tick_array::get_tick_array_address(&pool.address, current_array_start_index - (2 * ticks_in_array), &pool.program_id),
        ]
    } else {
        [
            mev::decoders::orca_decoders::tick_array::get_tick_array_address(&pool.address, current_array_start_index, &pool.program_id),
            mev::decoders::orca_decoders::tick_array::get_tick_array_address(&pool.address, current_array_start_index + ticks_in_array, &pool.program_id),
            mev::decoders::orca_decoders::tick_array::get_tick_array_address(&pool.address, current_array_start_index + (2 * ticks_in_array), &pool.program_id),
        ]
    };
    let (oracle_pda, _) = Pubkey::find_program_address(&[b"oracle", pool.address.as_ref()], &pool.program_id);
    let swap_discriminator: [u8; 8] = [248, 198, 158, 145, 225, 117, 135, 200];
    let sqrt_price_limit: u128 = if a_to_b { 4295048016 } else { 79226673515401279992447579055 };
    let mut instruction_data = Vec::new();
    instruction_data.extend_from_slice(&swap_discriminator);
    instruction_data.extend_from_slice(&amount_in_base_units.to_le_bytes());
    instruction_data.extend_from_slice(&0u64.to_le_bytes());
    instruction_data.extend_from_slice(&sqrt_price_limit.to_le_bytes());
    instruction_data.push(u8::from(true));
    instruction_data.push(u8::from(a_to_b));
    let swap_ix = Instruction {
        program_id: pool.program_id,
        accounts: vec![
            AccountMeta::new_readonly(spl_token::id(), false),
            AccountMeta::new_readonly(payer.pubkey(), true),
            AccountMeta::new(pool.address, false),
            AccountMeta::new(user_ata_for_token_a, false),
            AccountMeta::new(pool.vault_a, false),
            AccountMeta::new(user_ata_for_token_b, false),
            AccountMeta::new(pool.vault_b, false),
            AccountMeta::new(tick_array_addresses[0], false),
            AccountMeta::new(tick_array_addresses[1], false),
            AccountMeta::new(tick_array_addresses[2], false),
            AccountMeta::new_readonly(oracle_pda, false),
        ],
        data: instruction_data,
    };
    let instructions = vec![swap_ix];
    let transaction = Transaction::new_signed_with_payer(
        &instructions, Some(&payer.pubkey()), &[payer], rpc_client.get_latest_blockhash().await?,
    );

    // --- 3. EXÉCUTION & ANALYSE (par lecture de compte) ---
    println!("\n[3/3] Exécution de la simulation standard...");
    let sim_config = RpcSimulateTransactionConfig {
        sig_verify: false,
        replace_recent_blockhash: true,
        commitment: Some(rpc_client.commitment()),
        encoding: Some(UiTransactionEncoding::Base64),
        accounts: Some(solana_client::rpc_config::RpcSimulateTransactionAccountsConfig {
            encoding: Some(solana_account_decoder::UiAccountEncoding::Base64),
            addresses: vec![ user_destination_ata.to_string() ],
        }),
        ..Default::default()
    };
    let sim_response = rpc_client.simulate_transaction_with_config(&transaction, sim_config).await?;
    let sim_result = sim_response.value;
    if let Some(err) = sim_result.err {
        println!("!! ERREUR DE SIMULATION: {:?}", err);
        println!("!! LOGS: {:?}", sim_result.logs);
        bail!("La simulation a échoué.");
    }
    let initial_destination_balance = match rpc_client.get_token_account(&user_destination_ata).await {
        Ok(Some(acc)) => acc.token_amount.amount.parse::<u64>().unwrap_or(0),
        _ => 0,
    };
    let post_accounts = sim_result.accounts.ok_or_else(|| anyhow!("La simulation n'a pas retourné l'état des comptes."))?;
    let destination_account_state = post_accounts[0].as_ref().ok_or_else(|| anyhow!("L'état du compte de destination n'a pas été retourné."))?;
    let decoded_data = match &destination_account_state.data {
        UiAccountData::Binary(data_str, _) => base64::decode(data_str)?,
        _ => bail!("Format de données de compte inattendu."),
    };
    let token_account_data = SplTokenAccount::unpack(&decoded_data)?;
    let simulated_amount_out = token_account_data.amount.saturating_sub(initial_destination_balance);
    let ui_simulated_amount_out = simulated_amount_out as f64 / 10f64.powi(output_decimals as i32);

    println!("\n--- COMPARAISON (Whirlpool) ---");
    println!("Montant PRÉDIT (local)    : {}", ui_predicted_amount_out);
    println!("Montant SIMULÉ (on-chain) : {}", ui_simulated_amount_out);
    let difference = (ui_predicted_amount_out - ui_simulated_amount_out).abs();
    let difference_percent = if ui_simulated_amount_out > 0.0 { (difference / ui_simulated_amount_out) * 100.0 } else { 0.0 };
    println!("-> Différence relative: {:.6} %", difference_percent);
    if difference_percent < 0.1 {
        println!("✅ SUCCÈS ! Le décodeur Whirlpool est précis.");
    } else {
        println!("⚠️ ÉCHEC ! La différence est trop grande.");
    }
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




#[derive(BorshDeserialize, Debug)]
pub struct MeteoraSwapEvent {
    pub in_amount: u64,
    pub out_amount: u64,
    pub trade_fee: u64,
    pub protocol_fee: u64,
    pub host_fee: u64,
}

fn parse_meteora_swap_event_from_logs(logs: &[String]) -> Option<u64> {
    // Discriminateur pour l'événement "Swap", calculé depuis l'IDL: sha256("event:Swap")[..8]
    const SWAP_EVENT_DISCRIMINATOR: [u8; 8] = [81, 108, 227, 190, 205, 208, 10, 196];

    for log in logs {
        if let Some(data_str) = log.strip_prefix("Program data: ") {
            if let Ok(bytes) = base64::decode(data_str) {
                if bytes.len() > 8 && bytes.starts_with(&SWAP_EVENT_DISCRIMINATOR) {
                    let mut event_data: &[u8] = &bytes[8..];
                    if let Ok(event) = MeteoraSwapEvent::try_from_slice(&mut event_data) {
                        return Some(event.out_amount);
                    }
                }
            }
        }
    }
    None
}




async fn test_meteora_amm_with_simulation(rpc_client: &RpcClient, payer_keypair: &Keypair, current_timestamp: i64) -> Result<()> {
    const POOL_ADDRESS: &str = "ERgpKaq59Nnfm9YRVAAhnq16cZhHxGcDoDWCzXbhiaNw"; // WSOL-NOBODY
    const INPUT_MINT_STR: &str = "So11111111111111111111111111111111111111112"; // WSOL
    const INPUT_AMOUNT_UI: f64 = 0.05;

    println!("\n--- Test et Simulation (Lecture de Compte) Meteora AMM ({}) ---", POOL_ADDRESS);
    let pool_pubkey = Pubkey::from_str(POOL_ADDRESS)?;
    let input_mint_pubkey = Pubkey::from_str(INPUT_MINT_STR)?;

    // --- 1. Prédiction Locale ---
    println!("[1/3] Hydratation et prédiction locale...");
    let pool_account_data = rpc_client.get_account_data(&pool_pubkey).await?;
    let mut pool = meteora_amm::decode_pool(&pool_pubkey, &pool_account_data)?;
    meteora_amm::hydrate(&mut pool, rpc_client).await?;

    println!("\n--- DONNÉES DE DÉBOGAGE APRÈS HYDRATATION ---");
    println!("Type de Courbe: {:?}", pool.curve_type);

    // On copie les valeurs dans des variables locales avant de les afficher
    let trade_fee_num = pool.fees.trade_fee_numerator;
    let trade_fee_den = pool.fees.trade_fee_denominator;
    let protocol_fee_num = pool.fees.protocol_trade_fee_numerator;
    let protocol_fee_den = pool.fees.protocol_trade_fee_denominator;
    println!("Frais: trade_fee_numerator: {}, trade_fee_denominator: {}, protocol_trade_fee_numerator: {}, protocol_trade_fee_denominator: {}",
             trade_fee_num, trade_fee_den, protocol_fee_num, protocol_fee_den);

    println!("\n--- DÉTAILS VAULT A ({}) ---", pool.mint_a);
    let vault_a_total_amount = pool.vault_a_state.total_amount;
    let vault_a_locked_profit = pool.vault_a_state.locked_profit_tracker;
    println!("Réserve calculée (reserve_a): {}", pool.reserve_a);
    println!("Total tokens dans le vault (total_amount): {}", vault_a_total_amount);
    println!("Parts LP détenues par le pool (pool_a_vault_lp_amount): {}", pool.pool_a_vault_lp_amount);
    println!("Total parts LP du vault (vault_a_lp_supply): {}", pool.vault_a_lp_supply);
    println!("Profit verrouillé (locked_profit_tracker): {:?}", vault_a_locked_profit);

    println!("\n--- DÉTAILS VAULT B ({}) ---", pool.mint_b);
    let vault_b_total_amount = pool.vault_b_state.total_amount;
    let vault_b_locked_profit = pool.vault_b_state.locked_profit_tracker;
    println!("Réserve calculée (reserve_b): {}", pool.reserve_b);
    println!("Total tokens dans le vault (total_amount): {}", vault_b_total_amount);
    println!("Parts LP détenues par le pool (pool_b_vault_lp_amount): {}", pool.pool_b_vault_lp_amount);
    println!("Total parts LP du vault (vault_b_lp_supply): {}", pool.vault_b_lp_supply);
    println!("Profit verrouillé (locked_profit_tracker): {:?}", vault_b_locked_profit);
    println!("-------------------------------------------------\n");

    let (input_decimals, output_decimals, output_mint_pubkey) = if input_mint_pubkey == pool.mint_a {
        (pool.mint_a_decimals, pool.mint_b_decimals, pool.mint_b)
    } else {
        (pool.mint_b_decimals, pool.mint_a_decimals, pool.mint_a)
    };
    let amount_in_base_units = (INPUT_AMOUNT_UI * 10f64.powi(input_decimals as i32)) as u64;
    let predicted_amount_out = pool.get_quote(&input_mint_pubkey, amount_in_base_units, current_timestamp)?;
    let ui_predicted_amount_out = predicted_amount_out as f64 / 10f64.powi(output_decimals as i32);
    println!("-> PRÉDICTION LOCALE: {} UI", ui_predicted_amount_out);

    // --- 2. Préparation de la Transaction ---
    println!("\n[2/3] Préparation de la transaction de swap...");
    let payer_pubkey = payer_keypair.pubkey();
    let user_source_ata = get_associated_token_address(&payer_pubkey, &input_mint_pubkey);
    let user_destination_ata = get_associated_token_address(&payer_pubkey, &output_mint_pubkey);

    let swap_ix = pool.create_swap_instruction(
        &input_mint_pubkey,
        &user_source_ata,
        &user_destination_ata,
        &payer_pubkey,
        amount_in_base_units,
        0,
    )?;

    let recent_blockhash = rpc_client.get_latest_blockhash().await?;
    let transaction = Transaction::new_signed_with_payer(
        &[swap_ix],
        Some(&payer_pubkey),
        &[payer_keypair],
        recent_blockhash,
    );

    // --- 3. Simulation et Analyse par Lecture de Compte ---
    println!("\n[3/3] Exécution de la simulation avec lecture de compte...");

    // On récupère la balance du compte de destination AVANT la simulation.
    let initial_destination_balance = match rpc_client.get_token_account(&user_destination_ata).await {
        Ok(Some(acc)) => acc.token_amount.amount.parse::<u64>().unwrap_or(0),
        _ => 0, // Le compte n'existe pas ou erreur, la balance est 0.
    };
    println!("-> Balance initiale du compte de destination: {}", initial_destination_balance);

    // Configuration de la simulation pour qu'elle nous retourne l'état du compte de destination.
    let sim_config = RpcSimulateTransactionConfig {
        sig_verify: false,
        replace_recent_blockhash: true,
        commitment: Some(rpc_client.commitment()),
        encoding: Some(UiTransactionEncoding::Base64),
        accounts: Some(solana_client::rpc_config::RpcSimulateTransactionAccountsConfig {
            encoding: Some(solana_account_decoder::UiAccountEncoding::Base64),
            addresses: vec![ user_destination_ata.to_string() ],
        }),
        ..Default::default()
    };

    let sim_response = rpc_client.simulate_transaction_with_config(&transaction, sim_config).await?;
    let sim_result = sim_response.value;

    if let Some(err) = sim_result.err {
        println!("!! ERREUR DE SIMULATION: {:?}", err);
        println!("!! LOGS: {:?}", sim_result.logs);
        bail!("La simulation a échoué.");
    }

    // Extraction de la balance POST-simulation
    let post_accounts = sim_result.accounts.ok_or_else(|| anyhow!("La simulation n'a pas retourné l'état des comptes."))?;
    let destination_account_state = post_accounts.get(0).and_then(|acc| acc.as_ref()).ok_or_else(|| anyhow!("L'état du compte de destination n'a pas été retourné."))?;

    let decoded_data = match &destination_account_state.data {
        UiAccountData::Binary(data_str, _) => base64::decode(data_str)?,
        _ => bail!("Format de données de compte inattendu."),
    };

    let token_account_data = SplTokenAccount::unpack(&decoded_data)?;
    let post_simulation_balance = token_account_data.amount;
    println!("-> Balance finale du compte de destination (simulée): {}", post_simulation_balance);

    let simulated_amount_out = post_simulation_balance.saturating_sub(initial_destination_balance);

    let ui_simulated_amount_out = simulated_amount_out as f64 / 10f64.powi(output_decimals as i32);

    // --- 4. Comparaison ---
    println!("\n--- COMPARAISON FINALE (Meteora AMM) ---");
    println!("Montant PRÉDIT (local)             : {}", ui_predicted_amount_out);
    println!("Montant SIMULÉ (lecture de compte) : {}", ui_simulated_amount_out);
    let difference = (ui_predicted_amount_out - ui_simulated_amount_out).abs();
    let difference_percent = if ui_simulated_amount_out > 0.0 { (difference / ui_simulated_amount_out) * 100.0 } else { 0.0 };
    println!("-> Différence relative: {:.6} %", difference_percent);

    if difference_percent < 0.1 {
        println!("✅✅✅ SUCCÈS DÉFINITIF ! Le décodeur Meteora AMM est précis.");
    } else {
        println!("⚠️ ÉCHEC ! La différence est encore trop grande. Le problème est dans la formule de get_quote ou l'hydratation des réserves.");
    }

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




#[derive(BorshDeserialize, Debug)]
pub struct PumpBuyEvent {
    // Les champs doivent être définis dans l'ordre exact de l'IDL pour Borsh
    pub timestamp: i64,
    pub base_amount_out: u64, // <-- Le montant de sortie que nous voulons
    pub max_quote_amount_in: u64,
    pub user_base_token_reserves: u64,
    pub user_quote_token_reserves: u64,
    pub pool_base_token_reserves: u64,
    pub pool_quote_token_reserves: u64,
    pub quote_amount_in: u64,
    pub lp_fee_basis_points: u64,
    pub lp_fee: u64,
    pub protocol_fee_basis_points: u64,
    pub protocol_fee: u64,
    pub quote_amount_in_with_lp_fee: u64,
    pub user_quote_amount_in: u64,
    pub pool: Pubkey,
    pub user: Pubkey,
    pub user_base_token_account: Pubkey,
    pub user_quote_token_account: Pubkey,
    pub protocol_fee_recipient: Pubkey,
    pub protocol_fee_recipient_token_account: Pubkey,
    pub coin_creator: Pubkey,
    pub coin_creator_fee_basis_points: u64,
    pub coin_creator_fee: u64,
}


// Structure miroir de l'événement `SellEvent` de l'IDL de pump.fun
#[derive(BorshDeserialize, Debug)]
pub struct PumpSellEvent {
    pub timestamp: i64,
    pub base_amount_in: u64,
    pub min_quote_amount_out: u64, // Note: si u64 est suffisant, utilisez u64
    pub user_base_token_reserves: u64,
    pub user_quote_token_reserves: u64,
    pub pool_base_token_reserves: u64,
    pub pool_quote_token_reserves: u64,
    pub quote_amount_out: u64, // <-- Le montant de sortie que nous voulons
    pub lp_fee_basis_points: u64,
    pub lp_fee: u64,
    pub protocol_fee_basis_points: u64,
    pub protocol_fee: u64,
    pub quote_amount_out_without_lp_fee: u64,
    pub user_quote_amount_out: u64,
    pub pool: Pubkey,
    pub user: Pubkey,
    pub user_base_token_account: Pubkey,
    pub user_quote_token_account: Pubkey,
    pub protocol_fee_recipient: Pubkey,
    pub protocol_fee_recipient_token_account: Pubkey,
    pub coin_creator: Pubkey,
    pub coin_creator_fee_basis_points: u64,
    pub coin_creator_fee: u64,
}

// ... (vos fonctions de parsing existantes) ...

// Fonction pour parser spécifiquement cet événement depuis les logs
fn parse_pump_buy_event_from_logs(logs: &[String]) -> Option<u64> {
    const BUY_EVENT_DISCRIMINATOR: [u8; 8] = [103, 244, 82, 31, 44, 245, 119, 119];

    for log in logs {
        if let Some(data_str) = log.strip_prefix("Program data: ") {
            if let Ok(bytes) = base64::decode(data_str) {
                if bytes.len() > 8 && bytes.starts_with(&BUY_EVENT_DISCRIMINATOR) {
                    // Les données de l'événement commencent après le discriminateur de 8 octets.
                    let event_data = &bytes[8..];

                    // Selon l'IDL de BuyEvent, le premier champ est `timestamp` (i64, 8 octets).
                    // Le deuxième champ est `base_amount_out` (u64, 8 octets).
                    // Il se trouve donc à l'offset 8 par rapport au début des données de l'événement.
                    const OFFSET: usize = 8;

                    if event_data.len() >= OFFSET + 8 {
                        // On extrait les 8 octets correspondant au montant
                        let amount_bytes: [u8; 8] = event_data[OFFSET..OFFSET + 8].try_into().ok()?;
                        // On les convertit en u64 (little-endian)
                        return Some(u64::from_le_bytes(amount_bytes));
                    }
                }
            }
        }
    }
    None
}

fn parse_pump_sell_event_from_logs(logs: &[String]) -> Option<u64> {
    const SELL_EVENT_DISCRIMINATOR: [u8; 8] = [62, 47, 55, 10, 165, 3, 220, 42];

    for log in logs {
        if let Some(data_str) = log.strip_prefix("Program data: ") {
            if let Ok(bytes) = base64::decode(data_str) {
                if bytes.len() > 8 && bytes.starts_with(&SELL_EVENT_DISCRIMINATOR) {
                    let event_data = &bytes[8..];

                    // Selon l'IDL de SellEvent, les champs sont:
                    // 1. timestamp (i64, 8 octets)
                    // 2. base_amount_in (u64, 8 octets)
                    // 3. min_quote_amount_out (u64, 8 octets)
                    // ...
                    // 8. quote_amount_out (u64, 8 octets)
                    // L'offset est donc de 7 * 8 = 56 octets.
                    const OFFSET: usize = 56;

                    if event_data.len() >= OFFSET + 8 {
                        let amount_bytes: [u8; 8] = event_data[OFFSET..OFFSET + 8].try_into().ok()?;
                        return Some(u64::from_le_bytes(amount_bytes));
                    }
                }
            }
        }
    }
    None
}



async fn test_pump_amm_with_simulation(rpc_client: &RpcClient, payer_keypair: &Keypair, current_timestamp: i64) -> Result<()> {
    // --- SETUP DU TEST ---
    const POOL_ADDRESS: &str = "CLYFHhJfJjNPSMQv7byFeAsZ8x1EXQyYkGTPrNc2vc78"; // TADC-SOL
    // On simule un ACHAT de TADC en utilisant du SOL (WSOL)
    const INPUT_MINT_STR: &str = "So11111111111111111111111111111111111111112"; // WSOL Mint
    const INPUT_AMOUNT_UI: f64 = 0.05; // Le montant de SOL à dépenser

    println!("\n--- Test et Simulation (Standard, Simple) pump.fun AMM ({}) ---", POOL_ADDRESS);
    let pool_pubkey = Pubkey::from_str(POOL_ADDRESS)?;

    // --- 1. Prédiction Locale ---
    println!("[1/3] Hydratation et prédiction locale...");
    let pool_account_data = rpc_client.get_account_data(&pool_pubkey).await?;
    let mut pool = pump_decoders::amm::decode_pool(&pool_pubkey, &pool_account_data)?;
    pump_decoders::amm::hydrate(&mut pool, rpc_client).await?;

    let input_mint_pubkey = Pubkey::from_str(INPUT_MINT_STR)?;
    // Input est mint_b (SOL), Output est mint_a (TADC)
    let (input_decimals, output_decimals) = (pool.mint_b_decimals, pool.mint_a_decimals);
    let amount_in_base_units = (INPUT_AMOUNT_UI * 10f64.powi(input_decimals as i32)) as u64;

    // On appelle get_quote pour prédire combien de TADC on va recevoir
    let predicted_amount_out = pool.get_quote(&input_mint_pubkey, amount_in_base_units, current_timestamp)?;
    let ui_predicted_amount_out = predicted_amount_out as f64 / 10f64.powi(output_decimals as i32);
    println!("-> PRÉDICTION LOCALE (achat avec {} SOL): {} TADC", INPUT_AMOUNT_UI, ui_predicted_amount_out);

    // --- 2. Préparation de la Transaction Simple ---
    println!("\n[2/3] Préparation de la transaction d'ACHAT simple...");
    let swap_ix = pool.create_swap_instruction(
        &input_mint_pubkey,         // Le token d'entrée (SOL)
        &payer_keypair.pubkey(),
        amount_in_base_units,       // Le montant de SOL à dépenser
        predicted_amount_out,       // Le montant de TADC qu'on s'attend à recevoir
    )?;

    // La transaction ne contient qu'une seule instruction : le swap.
    let recent_blockhash = rpc_client.get_latest_blockhash().await?;
    let transaction = Transaction::new_signed_with_payer(
        &[swap_ix],
        Some(&payer_keypair.pubkey()),
        &[payer_keypair],
        recent_blockhash
    );

    // --- 3. Simulation et Analyse ---
    println!("\n[3/3] Exécution de la simulation standard...");
    let sim_result = rpc_client.simulate_transaction(&transaction).await?.value;

    if let Some(err) = sim_result.err {
        println!("LOGS DE SIMULATION DÉTAILLÉS:\n{:#?}", sim_result.logs);
        bail!("La simulation a échoué: {:?}", err);
    }

    let simulated_amount_out = sim_result.logs.as_ref()
        .and_then(|logs| parse_pump_buy_event_from_logs(logs))
        .ok_or_else(|| {
            if let Some(logs) = &sim_result.logs {
                println!("\n--- LOGS COMPLETS (Montant non trouvé) ---");
                logs.iter().for_each(|log| println!("{}", log));
            }
            anyhow!("Impossible de parser l'événement d'achat depuis les logs")
        })?;
    let ui_simulated_amount_out = simulated_amount_out as f64 / 10f64.powi(output_decimals as i32);

    // --- 4. Comparaison ---
    println!("\n--- COMPARAISON (pump.fun) ---");
    println!("Montant PRÉDIT (local)    : {}", ui_predicted_amount_out);
    println!("Montant SIMULÉ (on-chain) : {}", ui_simulated_amount_out);
    let difference = (ui_predicted_amount_out - ui_simulated_amount_out).abs();
    let difference_percent = if ui_simulated_amount_out > 0.0 { (difference / ui_simulated_amount_out) * 100.0 } else { 0.0 };
    println!("-> Différence relative: {:.6} %", difference_percent);

    if difference_percent < 0.1 {
        println!("✅ SUCCÈS ! Le décodeur pump.fun est précis.");
    } else {
        println!("⚠️ ÉCHEC ! La différence est trop grande.");
    }

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

    // On charge le vrai portefeuille depuis la clé privée dans le .env
    let payer_keypair = Keypair::from_base58_string(&config.payer_private_key);
    println!("-> Utilisation du portefeuille payeur : {}", payer_keypair.pubkey());

    let current_timestamp = get_timestamp(&rpc_client).await?;
    println!("-> Timestamp du cluster utilisé pour tous les tests: {}", current_timestamp);

    let args: Vec<String> = env::args().skip(1).collect();

    if args.is_empty() || args.contains(&"all".to_string()) {
        println!("Mode: Exécution de tous les tests.");
        if let Err(e) = test_ammv4(&rpc_client, current_timestamp).await { println!("!! AMMv4 a échoué: {}", e); }
        if let Err(e) = test_cpmm_with_simulation(&rpc_client, &payer_keypair, current_timestamp).await { println!("!! CPMM a échoué: {}", e); }
        if let Err(e) = test_clmm(&rpc_client, &payer_keypair, current_timestamp).await { println!("!! CLMM a échoué: {}", e); }
        if let Err(e) = test_launchpad(&rpc_client, current_timestamp).await { println!("!! Launchpad a échoué: {}", e); }
        if let Err(e) = test_meteora_amm_with_simulation(&rpc_client, &payer_keypair, current_timestamp).await { println!("!! Meteora AMM a échoué: {}", e); }
        if let Err(e) = test_damm_v2(&rpc_client, current_timestamp).await { println!("!! Meteora DAMM v2 a echoue: {}", e); }
        if let Err(e) = test_dlmm(&rpc_client, current_timestamp).await { println!("!! DLMM a échoué: {}", e); }
        if let Err(e) = test_whirlpool_with_simulation(&rpc_client, &payer_keypair, current_timestamp).await { println!("!! Whirlpool a échoué: {}", e); }
        if let Err(e) = test_orca_amm_v2(&rpc_client, current_timestamp).await { println!("!! Orca AMM V2 a échoué: {}", e); }
        if let Err(e) = test_orca_amm_v1(&rpc_client, current_timestamp).await { println!("!! Orca AMM V1 a échoué: {}", e); }
        if let Err(e) = test_pump_amm_with_simulation(&rpc_client, &payer_keypair, current_timestamp).await { println!("!! pump.fun AMM a échoué: {}", e); }
    } else {
        println!("Mode: Exécution des tests spécifiques: {:?}", args);
        for test_name in args {
            let result = match test_name.as_str() {
                "ammv4" => test_ammv4(&rpc_client, current_timestamp).await,
                "cpmm" => test_cpmm_with_simulation(&rpc_client, &payer_keypair, current_timestamp).await,
                "clmm" => test_clmm(&rpc_client, &payer_keypair, current_timestamp).await,
                "launchpad" => test_launchpad(&rpc_client, current_timestamp).await,
                "meteora_amm" => test_meteora_amm_with_simulation(&rpc_client, &payer_keypair, current_timestamp).await,
                "damm_v2" => test_damm_v2(&rpc_client, current_timestamp).await,
                "dlmm" => test_dlmm(&rpc_client, current_timestamp).await,
                "whirlpool" => test_whirlpool_with_simulation(&rpc_client, &payer_keypair, current_timestamp).await,
                "orca_amm_v2" => test_orca_amm_v2(&rpc_client, current_timestamp).await,
                "orca_amm_v1" => test_orca_amm_v1(&rpc_client, current_timestamp).await,
                "pump_amm" => test_pump_amm_with_simulation(&rpc_client, &payer_keypair, current_timestamp).await,
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