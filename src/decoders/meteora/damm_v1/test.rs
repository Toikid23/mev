use anyhow::{anyhow, bail, Result};
use solana_sdk::{
    pubkey::Pubkey,
    signature::Keypair,
    signer::Signer,
};
use std::str::FromStr;
use spl_associated_token_account::get_associated_token_address;
use solana_client::rpc_config::RpcSimulateTransactionConfig;
use solana_transaction_status::UiTransactionEncoding;
use solana_account_decoder::UiAccountData;
use spl_token::state::Account as SplTokenAccount;
use solana_program_pack::Pack;
use crate::decoders::pool_operations::UserSwapAccounts;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use solana_sdk::{
    transaction::{VersionedTransaction},
    message::VersionedMessage,
};
use tracing::{error, info,warn};

// --- Imports depuis notre propre crate ---
use crate::decoders::PoolOperations;
use crate::decoders::meteora::damm_v1::{
    decode_pool,
    hydrate,
};
use crate::rpc::ResilientRpcClient;

// --- Votre fonction de test (rendue publique) ---
pub async fn test_damm_v1_with_simulation(rpc_client: &ResilientRpcClient, payer_keypair: &Keypair, current_timestamp: i64) -> Result<()> {
    const POOL_ADDRESS: &str = "ERgpKaq59Nnfm9YRVAAhnq16cZhHxGcDoDWCzXbhiaNw"; // WSOL-NOBODY
    const INPUT_MINT_STR: &str = "So11111111111111111111111111111111111111112"; // WSOL
    const INPUT_AMOUNT_UI: f64 = 0.05;

    info!(pool_address = POOL_ADDRESS, "--- Test et Simulation (Lecture de Compte) Meteora DAMM_V1 ---");
    let pool_pubkey = Pubkey::from_str(POOL_ADDRESS)?;
    let input_mint_pubkey = Pubkey::from_str(INPUT_MINT_STR)?;

    // --- 1. Prédiction Locale ---
    info!("[1/3] Hydratation et prédiction locale...");
    let pool_account_data = rpc_client.get_account_data(&pool_pubkey).await?;
    let mut pool = decode_pool(&pool_pubkey, &pool_account_data)?;
    hydrate(&mut pool, rpc_client).await?;

    let (input_decimals, output_decimals, output_mint_pubkey) = if input_mint_pubkey == pool.mint_a {
        (pool.mint_a_decimals, pool.mint_b_decimals, pool.mint_b)
    } else {
        (pool.mint_b_decimals, pool.mint_a_decimals, pool.mint_a)
    };
    let amount_in_base_units = (INPUT_AMOUNT_UI * 10f64.powi(input_decimals as i32)) as u64;
    let predicted_amount_out = pool.get_quote(&input_mint_pubkey, amount_in_base_units, current_timestamp)?;
    let ui_predicted_amount_out = predicted_amount_out as f64 / 10f64.powi(output_decimals as i32);
    info!(predicted_amount_out = ui_predicted_amount_out, "Prédiction locale");

    // --- NOUVEAU BLOC DE VALIDATION ---
    info!("[VALIDATION] Lancement du test d'inversion mathématique...");
    if predicted_amount_out > 0 {
        let required_input_from_quote = pool.get_required_input(&output_mint_pubkey, predicted_amount_out, current_timestamp)?;

        info!(
            original_input = amount_in_base_units,
            predicted_output = predicted_amount_out,
            recalculated_input = required_input_from_quote,
            "Validation inverse"
        );

        if required_input_from_quote >= amount_in_base_units {
            let difference = required_input_from_quote.saturating_sub(amount_in_base_units);
            if difference <= 10 {
                info!(difference_lamports = difference, "✅ SUCCÈS : Le calcul inverse est cohérent.");
            } else {
                bail!("  -> ⚠️ ÉCHEC: La différence du calcul inverse est trop grande ({} lamports).", difference);
            }
        } else {
            bail!("  -> ⚠️ ÉCHEC: Le calcul inverse a produit un montant inférieur à l'original !");
        }
    } else {
        warn!("Le quote est de 0, test d'inversion sauté.");
    }
    // --- FIN DU BLOC DE VALIDATION ---

    // --- 2. Préparation de la Transaction ---
    info!("[2/3] Préparation de la transaction de swap...");

    let user_accounts = UserSwapAccounts {
        owner: payer_keypair.pubkey(),
        source: get_associated_token_address(&payer_keypair.pubkey(), &input_mint_pubkey),
        destination: get_associated_token_address(&payer_keypair.pubkey(), &output_mint_pubkey),
    };

    let swap_ix = pool.create_swap_instruction(
        &input_mint_pubkey,
        amount_in_base_units,
        0, // min_amount_out
        &user_accounts,
    )?;

    let recent_blockhash = rpc_client.get_latest_blockhash().await?;
    let transaction = VersionedTransaction::try_new(
        VersionedMessage::V0(solana_sdk::message::v0::Message::try_compile(
            &payer_keypair.pubkey(),
            &[swap_ix],
            &[], // Pas de LUT pour ce simple test
            recent_blockhash,
        )?),
        &[payer_keypair],
    )?;

    // --- 3. Simulation et Analyse par Lecture de Compte ---
    let user_destination_ata = user_accounts.destination;

    let initial_destination_balance = match rpc_client.get_account(&user_destination_ata).await {
        Ok(acc) => SplTokenAccount::unpack(&acc.data)?.amount,
        Err(_) => 0,
    };
    info!(initial_balance = initial_destination_balance, "Balance initiale du compte de destination");

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
        error!(simulation_error = ?err, logs = ?sim_result.logs, "ERREUR DE SIMULATION");
        bail!("La simulation a échoué.");
    }

    let post_accounts = sim_result.accounts.ok_or_else(|| anyhow!("La simulation n'a pas retourné l'état des comptes."))?;
    let destination_account_state = post_accounts.first().and_then(|acc| acc.as_ref()).ok_or_else(|| anyhow!("L'état du compte de destination n'a pas été retourné."))?;
    
    let decoded_data = match &destination_account_state.data {
        UiAccountData::Binary(data_str, _) => STANDARD.decode(data_str)?,
        _ => bail!("Format de données de compte inattendu."),
    };

    let token_account_data = SplTokenAccount::unpack(&decoded_data)?;
    let post_simulation_balance = token_account_data.amount;
    info!(final_balance = post_simulation_balance, "Balance finale du compte de destination (simulée)");

    let simulated_amount_out = post_simulation_balance.saturating_sub(initial_destination_balance);

    let ui_simulated_amount_out = simulated_amount_out as f64 / 10f64.powi(output_decimals as i32);

    // --- 4. Comparaison ---

    let difference = (ui_predicted_amount_out - ui_simulated_amount_out).abs();
    let difference_percent = if ui_simulated_amount_out > 0.0 { (difference / ui_simulated_amount_out) * 100.0 } else { 0.0 };
    info!(
        predicted = ui_predicted_amount_out,
        simulated = ui_simulated_amount_out,
        diff_percent = difference_percent,
        "--- COMPARAISON FINALE (Meteora DAMM_V1) ---"
    );

    if difference_percent < 0.1 {
        info!("✅✅✅ SUCCÈS DÉFINITIF ! Le décodeur Meteora DAMM_V1 est précis.");
    } else {
        error!("⚠️ ÉCHEC ! La différence est trop grande. Le problème est dans la formule de get_quote ou l'hydratation des réserves.");
    }

    Ok(())
}