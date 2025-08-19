use anyhow::{anyhow, bail, Result};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::{
    pubkey::Pubkey,
    signature::Keypair,
    signer::Signer,
    transaction::Transaction,
};
use std::str::FromStr;
use spl_associated_token_account::get_associated_token_address;

// --- Imports depuis notre propre crate ---
// On utilise 'crate::' car on est à l'intérieur de notre propre projet
use crate::decoders::raydium::cpmm::{
    decode_pool,
    hydrate,
    events,
};
use crate::decoders::PoolOperations;
use solana_client::rpc_config::RpcSimulateTransactionConfig;
use solana_transaction_status::UiTransactionEncoding;
use solana_account_decoder::UiAccountData;
use spl_token::state::Account as SplTokenAccount;
use solana_program_pack::Pack;


pub async fn test_cpmm_with_simulation(rpc_client: &RpcClient, payer_keypair: &Keypair, current_timestamp: i64) -> Result<()> {
    const POOL_ADDRESS: &str = "8ujpQXxnnWvRohU2oCe3eaSzoL7paU2uj3fEn4Zp72US";
    const INPUT_MINT_STR: &str = "So11111111111111111111111111111111111111112";
    const INPUT_AMOUNT_UI: f64 = 0.05;

    println!("\n--- Test et Simulation (Lecture de Compte) Raydium CPMM ({}) ---", POOL_ADDRESS);
    let pool_pubkey = Pubkey::from_str(POOL_ADDRESS)?;

    println!("[1/3] Hydratation et prédiction locale...");
    let pool_account_data = rpc_client.get_account_data(&pool_pubkey).await?;
    let mut pool = decode_pool(&pool_pubkey, &pool_account_data)?;
    hydrate(&mut pool, rpc_client).await?;

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

    println!("\n[3/3] Exécution de la simulation par lecture de compte...");

    // On récupère le solde AVANT
    let initial_destination_balance = match rpc_client.get_token_account(&user_destination_ata).await {
        Ok(Some(acc)) => acc.token_amount.amount.parse::<u64>().unwrap_or(0),
        _ => 0,
    };
    println!("-> Solde initial du compte de destination: {}", initial_destination_balance);

    // On configure la simulation pour qu'elle nous retourne l'état du compte APRES
    let sim_config = RpcSimulateTransactionConfig {
        sig_verify: false,
        replace_recent_blockhash: true,
        commitment: Some(rpc_client.commitment()),
        encoding: Some(UiTransactionEncoding::Base64),
        accounts: Some(solana_client::rpc_config::RpcSimulateTransactionAccountsConfig {
            encoding: Some(solana_account_decoder::UiAccountEncoding::Base64),
            addresses: vec![user_destination_ata.to_string()],
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

    // On décode l'état du compte APRES
    let post_accounts = sim_result.accounts.ok_or_else(|| anyhow!("La simulation n'a pas retourné l'état des comptes."))?;
    let destination_account_state = post_accounts.get(0).and_then(|acc| acc.as_ref()).ok_or_else(|| anyhow!("L'état du compte de destination n'a pas été retourné."))?;

    let decoded_data = match &destination_account_state.data {
        UiAccountData::Binary(data_str, _) => base64::decode(data_str)?,
        _ => bail!("Format de données de compte inattendu."),
    };

    let token_account_data = SplTokenAccount::unpack(&decoded_data)?;
    let post_simulation_balance = token_account_data.amount;
    println!("-> Solde final du compte de destination (simulé): {}", post_simulation_balance);

    // Le résultat est la simple différence
    let simulated_amount_out = post_simulation_balance.saturating_sub(initial_destination_balance);
    let ui_simulated_amount_out = simulated_amount_out as f64 / 10f64.powi(output_decimals as i32);

    println!("\n--- COMPARAISON (CPMM) ---");
    println!("Montant PRÉDIT (local)             : {}", ui_predicted_amount_out);
    println!("Montant SIMULÉ (lecture de compte) : {}", ui_simulated_amount_out);
    let difference = (ui_predicted_amount_out - ui_simulated_amount_out).abs();
    let difference_percent = if ui_simulated_amount_out > 0.0 { (difference / ui_simulated_amount_out) * 100.0 } else { 0.0 };
    println!("-> Différence relative: {:.6} %", difference_percent);

    if difference_percent < 0.1 { // On peut mettre un seuil plus petit, ex: 0.01
        println!("✅ SUCCÈS ! Le décodeur CPMM est précis.");
    } else {
        println!("⚠️ ÉCHEC ! La différence est trop grande.");
    }

    Ok(())
}