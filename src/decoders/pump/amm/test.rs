// DANS src/decoders/pump/amm/test.rs

use anyhow::{anyhow, bail, Result};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::{
    pubkey::Pubkey,
    signature::Keypair,
    signer::Signer,
    transaction::Transaction,
};
use std::str::FromStr;
use crate::decoders::pool_operations::UserSwapAccounts;
use spl_associated_token_account::get_associated_token_address_with_program_id;

use crate::decoders::PoolOperations;
use crate::decoders::pump::amm::{
    decode_pool,
    hydrate,
    events::parse_pump_buy_event_from_logs,
};
use crate::decoders::pump::amm::events;

pub async fn test_amm_with_simulation(rpc_client: &RpcClient, payer_keypair: &Keypair, current_timestamp: i64) -> anyhow::Result<()> {
    const POOL_ADDRESS: &str = "CLYFHhJfJjNPSMQv7byFeAsZ8x1EXQyYkGTPrNc2vc78";
    const OUTPUT_MINT_STR: &str = "666aGV6aC3s1j4T1p5a73f5s5g5H7J9k9L0m0N1p1q1r"; // Fausse adresse, l'important est d'avoir la variable
    const DESIRED_OUTPUT_AMOUNT_UI: f64 = 30000.0; // On fixe combien de tokens on VEUT recevoir.

    println!("\n--- Test et Simulation (FIXED-OUTPUT) pump.fun AMM ({}) ---", POOL_ADDRESS);
    let pool_pubkey = Pubkey::from_str(POOL_ADDRESS)?;

    println!("[1/3] Hydratation et prédiction du COÛT...");
    let pool_account_data = rpc_client.get_account_data(&pool_pubkey).await?;
    let mut pool = decode_pool(&pool_pubkey, &pool_account_data)?;
    hydrate(&mut pool, rpc_client).await?;

    let output_mint_pubkey = pool.mint_a;
    let (output_decimals, input_decimals) = (pool.mint_a_decimals, pool.mint_b_decimals);
    let desired_amount_out_base_units = (DESIRED_OUTPUT_AMOUNT_UI * 10f64.powi(output_decimals as i32)) as u64;

    // --- LA LOGIQUE INVERSÉE ---
    // On utilise get_required_input pour prédire le coût en SOL.
    let predicted_amount_in = pool.get_required_input(&output_mint_pubkey, desired_amount_out_base_units, current_timestamp)?;
    let ui_predicted_amount_in = predicted_amount_in as f64 / 10f64.powi(input_decimals as i32);
    println!("-> PRÉDICTION LOCALE (Coût pour {} tokens): {} SOL", DESIRED_OUTPUT_AMOUNT_UI, ui_predicted_amount_in);

    // --- 2. Préparation de la transaction ---
    println!("\n[2/3] Préparation de la transaction d'ACHAT (fixed-output)...");
    let user_accounts = UserSwapAccounts {
        owner: payer_keypair.pubkey(),
        source: get_associated_token_address_with_program_id(&payer_keypair.pubkey(), &pool.mint_b, &pool.mint_b_program),
        destination: get_associated_token_address_with_program_id(&payer_keypair.pubkey(), &pool.mint_a, &pool.mint_a_program),
    };

    // Le slippage s'applique sur l'input (on est prêt à payer un peu plus de SOL)
    const SLIPPAGE_PERCENT: f64 = 1.0;
    let max_amount_in = (predicted_amount_in as f64 * (1.0 + (SLIPPAGE_PERCENT / 100.0))) as u64;

    // L'instruction est un `buy` : on spécifie la sortie désirée et l'input max.
    let swap_ix = pool.create_swap_instruction(
        &pool.mint_b, // On paie avec le mint B (SOL)
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
    let transaction = Transaction::new_signed_with_payer(
        &instructions,
        Some(&payer_keypair.pubkey()),
        &[payer_keypair],
        recent_blockhash
    );

    // --- 3. Simulation et Analyse ---
    println!("\n[3/3] Exécution de la simulation...");
    let sim_result = rpc_client.simulate_transaction(&transaction).await?.value;

    if let Some(err) = sim_result.err {
        println!("LOGS DE SIMULATION DÉTAILLÉS:\n{:#?}", sim_result.logs);
        bail!("La simulation a échoué: {:?}", err);
    }

    // On extrait les deux informations de l'événement
    let logs = sim_result.logs.as_ref().ok_or_else(|| anyhow!("Logs de simulation vides"))?;

    let simulated_amount_out = parse_pump_buy_event_from_logs(logs)
        .ok_or_else(|| anyhow!("Impossible de parser le montant de sortie depuis les logs"))?;

    // --- NOUVELLE PARTIE : PARSING DU COÛT ---
    let simulated_amount_in = events::parse_pump_buy_event_cost_from_logs(logs)
        .ok_or_else(|| anyhow!("Impossible de parser le coût d'entrée depuis les logs"))?;

    let ui_simulated_amount_out = simulated_amount_out as f64 / 10f64.powi(output_decimals as i32);
    let ui_simulated_amount_in = simulated_amount_in as f64 / 10f64.powi(input_decimals as i32);

    println!("\n--- COMPARAISON FINALE (pump.fun) ---");
    println!("\n[VALIDATION - MONTANT REÇU]");
    println!("Montant DÉSIRÉ (local)     : {}", DESIRED_OUTPUT_AMOUNT_UI);
    println!("Montant REÇU (on-chain)    : {}", ui_simulated_amount_out);
    let diff_out_percent = if desired_amount_out_base_units > 0 {
        ((desired_amount_out_base_units as i128 - simulated_amount_out as i128).abs() as f64 / desired_amount_out_base_units as f64) * 100.0
    } else { 0.0 };
    println!("-> Différence relative (out): {:.6} %", diff_out_percent);

    println!("\n[VALIDATION - COÛT PAYÉ]");
    println!("Coût PRÉDIT (local)        : {:.9} SOL", ui_predicted_amount_in);
    println!("Coût RÉEL (on-chain)       : {:.9} SOL", ui_simulated_amount_in);
    let diff_in_percent = if predicted_amount_in > 0 {
        ((predicted_amount_in as i128 - simulated_amount_in as i128).abs() as f64 / predicted_amount_in as f64) * 100.0
    } else { 0.0 };
    println!("-> Différence relative (in) : {:.6} %", diff_in_percent);


    if diff_out_percent < 0.01 && diff_in_percent < 0.01 {
        println!("\n✅✅✅ SUCCÈS TOTAL ! Votre décodeur pump.fun est parfaitement calibré pour l'achat !");
    } else {
        bail!("⚠️ ÉCHEC FINAL ! Un des deux calculs est encore imprécis.");
    }

    Ok(())
}