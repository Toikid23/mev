// src/decoders/raydium/launchpad/tests.rs

use anyhow::{bail, Result};
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
use crate::decoders::pool_operations::UserSwapAccounts;

// --- Imports depuis notre propre crate ---
use crate::decoders::PoolOperations; // <-- NÉCESSAIRE pour print_quote_result
use crate::decoders::raydium::launchpad::{
    decode_pool,
    hydrate,
};

pub async fn test_launchpad(rpc_client: &RpcClient, payer: &Keypair, _current_timestamp: i64) -> Result<()> {
    const POOL_ADDRESS: &str = "DReGGrVpi1Czq5tC1Juu2NjZh1jtack4GwckuJqbQd7H"; // ZETA-USDC
    println!("\n--- Test et Simulation Raydium Launchpad ({}) ---", POOL_ADDRESS);
    let pool_pubkey = Pubkey::from_str(POOL_ADDRESS)?;

    let mut pool = decode_pool(&pool_pubkey, &rpc_client.get_account_data(&pool_pubkey).await?)?;
    hydrate(&mut pool, rpc_client).await?;

    let amount_in = 1_000_000; // 1 USDC
    let input_mint = pool.mint_b;
    let output_mint = pool.mint_a;

    println!("[1/2] Préparation de la simulation...");
    let user_accounts = UserSwapAccounts {
        owner: payer.pubkey(),
        source: get_associated_token_address(&payer.pubkey(), &input_mint),
        destination: get_associated_token_address(&payer.pubkey(), &output_mint),
    };

    let swap_ix = pool.create_swap_instruction(&input_mint, amount_in, 0, &user_accounts)?;

    let transaction = Transaction::new_signed_with_payer(
        &[swap_ix], Some(&payer.pubkey()), &[payer], rpc_client.get_latest_blockhash().await?,
    );

    println!("[2/2] Exécution de la simulation...");
    let sim_result = rpc_client.simulate_transaction(&transaction).await?;

    if let Some(err) = sim_result.value.err {
        bail!("La simulation a échoué: {:?}\nLogs: {:?}", err, sim_result.value.logs);
    }

    println!("✅ SUCCÈS ! La simulation de swap Launchpad a réussi.");
    // NOTE: On ne peut pas comparer le montant car events.rs est vide.
    // L'objectif ici est de valider que la création de l'instruction est correcte.
    Ok(())
}