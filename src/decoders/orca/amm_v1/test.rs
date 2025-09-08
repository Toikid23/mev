use anyhow::{bail, Result};
use solana_sdk::{
    pubkey::Pubkey,
    signature::Keypair,
    signer::Signer,
    transaction::VersionedTransaction,
    message::VersionedMessage,
};
use std::str::FromStr;
use spl_associated_token_account::get_associated_token_address;
use crate::decoders::pool_operations::{PoolOperations, UserSwapAccounts};
use crate::decoders::orca::amm_v1::{decode_pool, hydrate};
use crate::rpc::ResilientRpcClient;

pub async fn test_orca_amm_v1(rpc_client: &ResilientRpcClient, payer: &Keypair, _current_timestamp: i64) -> Result<()> {
    const POOL_ADDRESS: &str = "6fTRDD7sYxCN7oyoSQaN1AWC3P2m8A6gVZzGrpej9DvL"; // SOL/USDC
    println!("\n--- Test et Simulation Orca AMM V1 ({}) ---", POOL_ADDRESS);

    let pool_pubkey = Pubkey::from_str(POOL_ADDRESS)?;

    let account_data = rpc_client.get_account_data(&pool_pubkey).await?;
    let mut pool = decode_pool(&pool_pubkey, &account_data)?;
    println!("-> Décodage initial réussi. Courbe type: {}.", pool.curve_type);

    println!("[1/2] Hydratation...");
    hydrate(&mut pool, rpc_client).await?;

    let amount_in_sol = 1_000_000_000; // 1 SOL
    let input_mint = pool.mint_a;

    println!("\n[2/2] Préparation et simulation pour VENDRE 1 SOL...");

    let user_accounts = UserSwapAccounts {
        owner: payer.pubkey(),
        source: get_associated_token_address(&payer.pubkey(), &pool.mint_a),
        destination: get_associated_token_address(&payer.pubkey(), &pool.mint_b),
    };

    let swap_ix = pool.create_swap_instruction(&input_mint, amount_in_sol, 0, &user_accounts)?;

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

    let sim_result = rpc_client.simulate_transaction(&transaction).await?;

    if let Some(err) = sim_result.value.err {
        bail!("La simulation a échoué: {:?}\nLogs: {:?}", err, sim_result.value.logs);
    }

    println!("✅ SUCCÈS ! La simulation de swap Orca AMM V1 a réussi.");
    Ok(())
}