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
use crate::decoders::pool_operations::UserSwapAccounts;
use crate::decoders::PoolOperations;

// --- Imports depuis notre propre crate ---
use crate::decoders::orca::whirlpool::{
    decode_pool,
    hydrate,
    tick_array,
};

pub async fn test_whirlpool_with_simulation(rpc_client: &RpcClient, payer: &Keypair, _current_timestamp: i64) -> Result<()> {
    const POOL_ADDRESS: &str = "Czfq3xZZDmsdGdUyrNLtRhGc47cXcZtLG4crryfu44zE";
    const INPUT_MINT_STR: &str = "So11111111111111111111111111111111111111112";
    const INPUT_AMOUNT_UI: f64 = 0.05;

    println!("\n--- Test et Simulation Orca Whirlpool ({}) ---", POOL_ADDRESS);
    let pool_pubkey = Pubkey::from_str(POOL_ADDRESS)?;
    let input_mint_pubkey = Pubkey::from_str(INPUT_MINT_STR)?;

    // --- 1. PRÉDICTION LOCALE (inchangé) ---
    println!("[1/3] Hydratation et prédiction locale...");
    let pool_account_data = rpc_client.get_account_data(&pool_pubkey).await?;
    let mut pool = decode_pool(&pool_pubkey, &pool_account_data)?;
    hydrate(&mut pool, rpc_client).await?;
    let (input_decimals, output_decimals, _) = if input_mint_pubkey == pool.mint_a {
        (pool.mint_a_decimals, pool.mint_b_decimals, pool.mint_b)
    } else {
        (pool.mint_b_decimals, pool.mint_a_decimals, pool.mint_a)
    };
    let amount_in_base_units = (INPUT_AMOUNT_UI * 10f64.powi(input_decimals as i32)) as u64;
    let predicted_amount_out = pool.get_quote_with_rpc(&input_mint_pubkey, amount_in_base_units, rpc_client).await?;
    let ui_predicted_amount_out = predicted_amount_out as f64 / 10f64.powi(output_decimals as i32);
    println!("-> PRÉDICTION LOCALE: {} UI", ui_predicted_amount_out);

    // --- 2. PRÉPARATION DE LA SIMULATION (maintenant simplifié) ---
    println!("\n[2/3] Préparation de la simulation...");

    // On a juste besoin de définir la struct UserSwapAccounts
    let user_accounts = UserSwapAccounts {
        owner: payer.pubkey(),
        source: get_associated_token_address(&payer.pubkey(), &input_mint_pubkey),
        destination: get_associated_token_address(&payer.pubkey(), if input_mint_pubkey == pool.mint_a { &pool.mint_b } else { &pool.mint_a }),
    };

    // L'appel unifié gère toute la complexité
    let swap_ix = pool.create_swap_instruction(
        &input_mint_pubkey,
        amount_in_base_units,
        0, // min_amount_out
        &user_accounts
    )?;

    let transaction = Transaction::new_signed_with_payer(
        &[swap_ix], Some(&payer.pubkey()), &[payer], rpc_client.get_latest_blockhash().await?,
    );

    // --- 3. EXÉCUTION & ANALYSE (votre code original, inchangé) ---
    println!("\n[3/3] Exécution de la simulation standard...");
    let user_destination_ata = user_accounts.destination; // On récupère l'ATA de destination
    // ... (le reste de votre logique de simulation et de comparaison est parfait et reste ici)
    // ...
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