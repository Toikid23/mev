use anyhow::{anyhow, bail, Result};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::{
    instruction::{Instruction, AccountMeta},
    pubkey::Pubkey,
    signature::Keypair,
    signer::Signer,
    transaction::Transaction,
};
use std::str::FromStr;
use spl_associated_token_account::get_associated_token_address;
use solana_client::rpc_config::RpcSimulateTransactionConfig;
use solana_transaction_status::UiTransactionEncoding;
use solana_account_decoder::UiAccountData;
use spl_token::state::Account as SplTokenAccount;
use solana_program_pack::Pack;

// --- Imports depuis notre propre crate ---
use crate::decoders::orca::whirlpool::{
    decode_pool,
    hydrate,
    tick_array,
};

// --- Votre fonction de test (rendue publique) ---
pub async fn test_whirlpool_with_simulation(rpc_client: &RpcClient, payer: &Keypair, _current_timestamp: i64) -> Result<()> {
    const POOL_ADDRESS: &str = "Czfq3xZZDmsdGdUyrNLtRhGc47cXcZtLG4crryfu44zE";
    const INPUT_MINT_STR: &str = "So11111111111111111111111111111111111111112";
    const INPUT_AMOUNT_UI: f64 = 0.05;

    println!("\n--- Test et Simulation Orca Whirlpool ({}) ---", POOL_ADDRESS);
    let pool_pubkey = Pubkey::from_str(POOL_ADDRESS)?;
    let input_mint_pubkey = Pubkey::from_str(INPUT_MINT_STR)?;

    // --- 1. PRÉDICTION LOCALE ---
    println!("[1/3] Hydratation et prédiction locale...");
    let pool_account_data = rpc_client.get_account_data(&pool_pubkey).await?;

    let mut pool = decode_pool(&pool_pubkey, &pool_account_data)?;
    hydrate(&mut pool, rpc_client).await?;

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

    // --- 2. PRÉPARATION DE LA SIMULATION ---
    println!("\n[2/3] Préparation de la simulation (en supposant que les ATAs existent)...");
    let user_ata_for_token_a = get_associated_token_address(&payer.pubkey(), &pool.mint_a);
    let user_ata_for_token_b = get_associated_token_address(&payer.pubkey(), &pool.mint_b);
    let a_to_b = input_mint_pubkey == pool.mint_a;
    let (user_source_ata, user_destination_ata) = if a_to_b { (user_ata_for_token_a, user_ata_for_token_b) } else { (user_ata_for_token_b, user_ata_for_token_a) };

    let ticks_in_array = (tick_array::TICK_ARRAY_SIZE as i32) * (pool.tick_spacing as i32);
    let current_array_start_index = tick_array::get_start_tick_index(pool.tick_current_index, pool.tick_spacing);
    let tick_array_addresses: [Pubkey; 3] = if a_to_b {
        [
            tick_array::get_tick_array_address(&pool.address, current_array_start_index, &pool.program_id),
            tick_array::get_tick_array_address(&pool.address, current_array_start_index - ticks_in_array, &pool.program_id),
            tick_array::get_tick_array_address(&pool.address, current_array_start_index - (2 * ticks_in_array), &pool.program_id),
        ]
    } else {
        [
            tick_array::get_tick_array_address(&pool.address, current_array_start_index, &pool.program_id),
            tick_array::get_tick_array_address(&pool.address, current_array_start_index + ticks_in_array, &pool.program_id),
            tick_array::get_tick_array_address(&pool.address, current_array_start_index + (2 * ticks_in_array), &pool.program_id),
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