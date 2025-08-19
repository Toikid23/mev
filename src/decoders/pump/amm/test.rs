use anyhow::{anyhow, bail, Result};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::{
    pubkey::Pubkey,
    signature::Keypair,
    signer::Signer,
    transaction::Transaction,
};
use std::str::FromStr;

// --- Imports depuis notre propre crate ---
use crate::decoders::PoolOperations; // <-- NÉCESSAIRE pour pool.get_quote()
use crate::decoders::pump::amm::{
    decode_pool,
    hydrate,
    events::parse_pump_buy_event_from_logs,
};


pub async fn test_amm_with_simulation(rpc_client: &RpcClient, payer_keypair: &Keypair, current_timestamp: i64) -> anyhow::Result<()> {
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
    let mut pool = decode_pool(&pool_pubkey, &pool_account_data)?;
    hydrate(&mut pool, rpc_client).await?;

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