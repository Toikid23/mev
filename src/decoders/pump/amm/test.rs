use anyhow::{anyhow, bail};
use solana_sdk::{
    pubkey::Pubkey,
    signature::Keypair,
    signer::Signer,
    transaction::VersionedTransaction,
    message::VersionedMessage,
};
use std::str::FromStr;
use crate::decoders::pool_operations::UserSwapAccounts;
use spl_associated_token_account::get_associated_token_address_with_program_id;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use crate::rpc::ResilientRpcClient;

use crate::decoders::PoolOperations;
use crate::decoders::pump::amm::{
    decode_pool,
    hydrate,
    events::{PumpBuyEvent},
};
use borsh::BorshDeserialize;

pub async fn test_amm_with_simulation(rpc_client: &ResilientRpcClient, payer_keypair: &Keypair, current_timestamp: i64) -> anyhow::Result<()> {
    const POOL_ADDRESS: &str = "CLYFHhJfJjNPSMQv7byFeAsZ8x1EXQyYkGTPrNc2vc78";
    const DESIRED_OUTPUT_AMOUNT_UI: f64 = 30000.0;

    println!("\n--- Test et Simulation (FIXED-OUTPUT) pump.fun AMM ({}) ---", POOL_ADDRESS);
    let pool_pubkey = Pubkey::from_str(POOL_ADDRESS)?;

    println!("[1/3] Hydratation et prédiction du COÛT...");
    let pool_account_data = rpc_client.get_account_data(&pool_pubkey).await?;
    let mut pool = decode_pool(&pool_pubkey, &pool_account_data)?;
    hydrate(&mut pool, rpc_client).await?;

    let output_mint_pubkey = pool.mint_a;
    let (output_decimals, input_decimals) = (pool.mint_a_decoded.decimals, pool.mint_b_decoded.decimals);
    let desired_amount_out_base_units = (DESIRED_OUTPUT_AMOUNT_UI * 10f64.powi(output_decimals as i32)) as u64;

    let predicted_amount_in = pool.get_required_input(&output_mint_pubkey, desired_amount_out_base_units, current_timestamp)?;
    let ui_predicted_amount_in = predicted_amount_in as f64 / 10f64.powi(input_decimals as i32);
    // On ne log plus ici, les logs sont maintenant dans la fonction elle-même

    println!("\n[2/3] Préparation de la transaction d'ACHAT (fixed-output)...");
    let user_accounts = UserSwapAccounts {
        owner: payer_keypair.pubkey(),
        source: get_associated_token_address_with_program_id(&payer_keypair.pubkey(), &pool.mint_b, &pool.mint_b_program),
        destination: get_associated_token_address_with_program_id(&payer_keypair.pubkey(), &pool.mint_a, &pool.mint_a_program),
    };

    const SLIPPAGE_PERCENT: f64 = 5.0; // Augmenter le slippage pour le test
    let max_amount_in = (predicted_amount_in as f64 * (1.0 + (SLIPPAGE_PERCENT / 100.0))) as u64;

    let swap_ix = pool.create_swap_instruction(
        &pool.mint_b,
        max_amount_in,
        desired_amount_out_base_units,
        &user_accounts,
    )?;

    let mut instructions = Vec::new();
    if rpc_client.get_account(&user_accounts.destination).await.is_err() {
        instructions.push(
            spl_associated_token_account::instruction::create_associated_token_account(
                &payer_keypair.pubkey(), &user_accounts.owner, &pool.mint_a, &pool.mint_a_program,
            ),
        );
    }
    instructions.push(swap_ix);

    let recent_blockhash = rpc_client.get_latest_blockhash().await?;
    // --- MODIFICATION ICI ---
    let transaction = VersionedTransaction::try_new(
        VersionedMessage::V0(solana_sdk::message::v0::Message::try_compile(
            &payer_keypair.pubkey(),
            &instructions,
            &[], // Pas de LUT
            recent_blockhash,
        )?),
        &[payer_keypair],
    )?;

    println!("\n[3/3] Exécution de la simulation...");
    let sim_result = rpc_client.simulate_transaction(&transaction).await?.value;

    if let Some(err) = sim_result.err {
        println!("LOGS DE SIMULATION DÉTAILLÉS:\n{:#?}", sim_result.logs);
        bail!("La simulation a échoué: {:?}", err);
    }

    let logs = sim_result.logs.as_ref().ok_or_else(|| anyhow!("Logs de simulation vides"))?;

    // --- DÉCODAGE COMPLET DE L'ÉVÉNEMENT ---
    const BUY_EVENT_DISCRIMINATOR: [u8; 8] = [103, 244, 82, 31, 44, 245, 119, 119];
    let mut onchain_event: Option<PumpBuyEvent> = None;
    for log in logs {
        if let Some(data_str) = log.strip_prefix("Program data: ") {
            if let Ok(bytes) = STANDARD.decode(data_str) {
                if bytes.len() > 8 && bytes.starts_with(&BUY_EVENT_DISCRIMINATOR) {
                    let mut event_data = &bytes[8..];
                    onchain_event = PumpBuyEvent::deserialize(&mut event_data).ok();
                    break;
                }
            }
        }
    }

    let event = onchain_event.ok_or_else(|| anyhow!("Impossible de parser l'événement d'achat complet"))?;

    println!("\n--- [ON-CHAIN EVENT DEBUG] ---");
    println!("  - Coût NET (quote_amount_in) : {}", event.quote_amount_in);
    println!("  - Frais LP (lamports)        : {}", event.lp_fee);
    println!("  - Frais Prot (lamports)      : {}", event.protocol_fee);
    println!("  - Frais Créat. (lamports)      : {}", event.coin_creator_fee);
    println!("  - Coût Total PAYÉ            : {}", event.user_quote_amount_in);
    println!("  - Sortie Reçue               : {}", event.base_amount_out);
    println!("--------------------------------");

    let simulated_amount_out = event.base_amount_out;
    let simulated_amount_in = event.user_quote_amount_in;

    let ui_simulated_amount_out = simulated_amount_out as f64 / 10f64.powi(output_decimals as i32);
    let ui_simulated_amount_in = simulated_amount_in as f64 / 10f64.powi(input_decimals as i32);

    println!("\n--- COMPARAISON FINALE (pump.fun) ---");
    println!("\n[VALIDATION - MONTANT REÇU]");
    println!("Montant DÉSIRÉ (local)     : {}", DESIRED_OUTPUT_AMOUNT_UI);
    println!("Montant REÇU (on-chain)    : {}", ui_simulated_amount_out);
    let diff_out_percent = ((desired_amount_out_base_units as i128 - simulated_amount_out as i128).abs() as f64 / desired_amount_out_base_units as f64) * 100.0;
    println!("-> Différence relative (out): {:.6} %", diff_out_percent);

    println!("\n[VALIDATION - COÛT PAYÉ]");
    println!("Coût PRÉDIT (local)        : {:.9} SOL", ui_predicted_amount_in);
    println!("Coût RÉEL (on-chain)       : {:.9} SOL", ui_simulated_amount_in);
    let diff_in_percent = ((predicted_amount_in as i128 - simulated_amount_in as i128).abs() as f64 / predicted_amount_in as f64) * 100.0;
    println!("-> Différence relative (in) : {:.6} %", diff_in_percent);

    if diff_out_percent < 0.01 && diff_in_percent < 0.01 {
        println!("\n✅✅✅ SUCCÈS TOTAL ! Votre décodeur pump.fun est parfaitement calibré pour l'achat !");
    } else {
        bail!("⚠️ ÉCHEC FINAL ! Un des deux calculs est encore imprécis.");
    }

    Ok(())
}