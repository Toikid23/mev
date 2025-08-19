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
use crate::decoders::raydium::clmm::{
    decode_pool,
    hydrate,
    events::parse_swap_event_from_logs,
    tick_array,
};

pub async fn test_clmm(rpc_client: &RpcClient, payer: &Keypair, current_timestamp: i64) -> Result<()> {
    const POOL_ADDRESS: &str = "YrrUStgPugDp8BbfosqDeFssen6sA75ZS1QJvgnHtmY";
    const PROGRAM_ID: &str = "CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK";
    const INPUT_MINT_STR: &str = "So11111111111111111111111111111111111111112";
    const INPUT_AMOUNT_UI: f64 = 0.1;

    println!("\n--- Test et Simulation (VRAI COMPTE) Raydium CLMM ({}) ---", POOL_ADDRESS);
    let pool_pubkey = Pubkey::from_str(POOL_ADDRESS)?;
    let program_pubkey = Pubkey::from_str(PROGRAM_ID)?;

    println!("[1/3] Hydratation et prédiction locale...");
    let pool_account_data = rpc_client.get_account_data(&pool_pubkey).await?;
    let mut pool = decode_pool(&pool_pubkey, &pool_account_data, &program_pubkey)?;
    hydrate(&mut pool, rpc_client).await?;
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
    let ticks_in_array = (tick_array::TICK_ARRAY_SIZE as i32) * (pool.tick_spacing as i32);

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
        tick_array::get_tick_array_address(&pool.address, index, &pool.program_id)
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