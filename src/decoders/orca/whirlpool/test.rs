// DANS : src/decoders/orca/whirlpool/test.rs

use anyhow::{anyhow, bail, Result};
use solana_sdk::{
    pubkey::Pubkey,
    signature::Keypair,
    signer::Signer,
    transaction::VersionedTransaction,
    message::VersionedMessage,
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
use base64::{engine::general_purpose::STANDARD, Engine as _};
use tracing::{error, info,warn};

// Imports depuis notre propre crate
use crate::decoders::orca::whirlpool::{decode_pool, hydrate};
use crate::rpc::ResilientRpcClient; // <-- NOUVEL IMPORT

// --- La signature de la fonction change ---
pub async fn test_whirlpool_with_simulation(rpc_client: &ResilientRpcClient, payer: &Keypair, current_timestamp: i64) -> Result<()> {
    const POOL_ADDRESS: &str = "Czfq3xZZDmsdGdUyrNLtRhGc47cXcZtLG4crryfu44zE";
    const INPUT_MINT_STR: &str = "So11111111111111111111111111111111111111112";
    const INPUT_AMOUNT_UI: f64 = 0.05;

    info!(pool_address = POOL_ADDRESS, "--- Test et Simulation Orca Whirlpool ---");
    let pool_pubkey = Pubkey::from_str(POOL_ADDRESS)?;
    let input_mint_pubkey = Pubkey::from_str(INPUT_MINT_STR)?;

    info!("[1/3] Hydratation et prédiction locale...");
    // Les appels RPC utilisent maintenant notre client résilient
    let pool_account_data = rpc_client.get_account_data(&pool_pubkey).await?;
    let mut pool = decode_pool(&pool_pubkey, &pool_account_data)?;
    hydrate(&mut pool, rpc_client).await?; // Cet appel va maintenant fonctionner

    // Le reste de la logique de prédiction est inchangé
    let (input_decimals, output_decimals, output_mint_pubkey) = if input_mint_pubkey == pool.mint_a {
        (pool.mint_a_decimals, pool.mint_b_decimals, pool.mint_b)
    } else {
        (pool.mint_b_decimals, pool.mint_a_decimals, pool.mint_a)
    };
    let amount_in_base_units = (INPUT_AMOUNT_UI * 10f64.powi(input_decimals as i32)) as u64;
    let predicted_amount_out = pool.get_quote(&input_mint_pubkey, amount_in_base_units, current_timestamp)?;
    let ui_predicted_amount_out = predicted_amount_out as f64 / 10f64.powi(output_decimals as i32);
    info!(predicted_amount_out = ui_predicted_amount_out, "Prédiction locale");

    // [VALIDATION INVERSE] ... (code inchangé)
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
            if difference <= 100 {
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

    info!("[2/3] Préparation de la simulation...");
    let user_accounts = UserSwapAccounts {
        owner: payer.pubkey(),
        source: get_associated_token_address(&payer.pubkey(), &input_mint_pubkey),
        destination: get_associated_token_address(&payer.pubkey(), &output_mint_pubkey),
    };
    let swap_ix = pool.create_swap_instruction(&input_mint_pubkey, amount_in_base_units, 0, &user_accounts)?;

    let recent_blockhash = rpc_client.get_latest_blockhash().await?;
    // --- MODIFICATION ICI ---
    let transaction = VersionedTransaction::try_new(
        VersionedMessage::V0(solana_sdk::message::v0::Message::try_compile(
            &payer.pubkey(),
            &[swap_ix],
            &[], // Pas de LUT
            recent_blockhash,
        )?),
        &[payer],
    )?;

    info!("[3/3] Exécution de la simulation standard...");
    let user_destination_ata = user_accounts.destination;

    // --- AMÉLIORATION DE LA LECTURE DU SOLDE ---
    let initial_destination_balance = match rpc_client.get_account(&user_destination_ata).await {
        Ok(acc) => SplTokenAccount::unpack(&acc.data)?.amount,
        Err(_) => 0,
    };

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

    // Cet appel va maintenant utiliser notre wrapper
    let sim_response = rpc_client.simulate_transaction_with_config(&transaction, sim_config).await?;
    let sim_result = sim_response.value;

    // Le reste du code de comparaison est inchangé
    if let Some(err) = sim_result.err {
        error!(simulation_error = ?err, logs = ?sim_result.logs, "ERREUR DE SIMULATION");
        bail!("La simulation a échoué.");
    }
    let post_accounts = sim_result.accounts.ok_or_else(|| anyhow!("La simulation n'a pas retourné l'état des comptes."))?;
    let destination_account_state = post_accounts[0].as_ref().ok_or_else(|| anyhow!("L'état du compte de destination n'a pas été retourné."))?;
    let decoded_data = match &destination_account_state.data {
        UiAccountData::Binary(data_str, _) => STANDARD.decode(data_str)?,
        _ => bail!("Format de données de compte inattendu."),
    };
    let token_account_data = SplTokenAccount::unpack(&decoded_data)?;
    let simulated_amount_out = token_account_data.amount.saturating_sub(initial_destination_balance);
    let ui_simulated_amount_out = simulated_amount_out as f64 / 10f64.powi(output_decimals as i32);

    let difference = (ui_predicted_amount_out - ui_simulated_amount_out).abs();
    let difference_percent = if ui_simulated_amount_out > 0.0 { (difference / ui_simulated_amount_out) * 100.0 } else { 0.0 };
    info!(
        predicted = ui_predicted_amount_out,
        simulated = ui_simulated_amount_out,
        diff_percent = difference_percent,
        "--- COMPARAISON (Whirlpool) ---"
    );
    if difference_percent < 0.1 {
        info!("✅ SUCCÈS ! Le décodeur Whirlpool est précis.");
    } else {
        error!("⚠️ ÉCHEC ! La différence est trop grande.");
    }

    Ok(())
}