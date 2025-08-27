// Fichier : src/decoders/raydium/stable_swap/test.rs (Version Complète)

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

// Imports nécessaires pour le test
use crate::decoders::pool_operations::{PoolOperations, UserSwapAccounts};
use crate::decoders::raydium::stable_swap::{decode_pool_info, decode_model_data, DecodedStableSwapPool};

// --- Remplacer toute la fonction de test placeholder ---
pub async fn test_stable_swap(rpc_client: &RpcClient, payer: &Keypair, _current_timestamp: i64) -> Result<()> {
    const POOL_ADDRESS: &str = "58Yvj55wLMA8sMD4oZ2f2K1sC3t25a1grGH2sNqt7WfL"; // USDC-USDT
    println!("\n--- Test et Simulation Raydium Stable Swap ({}) ---", POOL_ADDRESS);

    // --- Hydratation spécifique au Stable Swap ---
    let pool_pubkey = Pubkey::from_str(POOL_ADDRESS)?;
    let pool_account_data = rpc_client.get_account_data(&pool_pubkey).await?;
    let (pool_info, total_fee) = decode_pool_info(&pool_pubkey, &pool_account_data)?;

    let model_data_account_data = rpc_client.get_account_data(&pool_info.model_data_key).await?;
    let amp = decode_model_data(&model_data_account_data)?;

    // NOTE: get_token_account_balance est déprécié, on utilise get_account + unpack pour la robustesse
    let vault_a_data = rpc_client.get_account_data(&pool_info.coin_vault).await?;
    let reserve_a = u64::from_le_bytes(vault_a_data[64..72].try_into()?);
    let vault_b_data = rpc_client.get_account_data(&pool_info.pc_vault).await?;
    let reserve_b = u64::from_le_bytes(vault_b_data[64..72].try_into()?);

    let mut pool = DecodedStableSwapPool {
        address: pool_pubkey,
        mint_a: pool_info.coin_mint,
        mint_b: pool_info.pc_mint,
        vault_a: pool_info.coin_vault,
        vault_b: pool_info.pc_vault,
        model_data_account: pool_info.model_data_key,
        total_fee_percent: total_fee,
        amp,
        reserve_a,
        reserve_b,
        last_swap_timestamp: 0,
    };

    let amount_in = 1_000_000; // 1 USDC (6 décimales)
    let input_mint = pool.mint_a;

    println!("[1/2] Préparation de la simulation...");
    let user_accounts = UserSwapAccounts {
        owner: payer.pubkey(),
        source: get_associated_token_address(&payer.pubkey(), &pool.mint_a),
        destination: get_associated_token_address(&payer.pubkey(), &pool.mint_b),
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

    println!("✅ SUCCÈS ! La simulation de swap Stable Swap a réussi.");
    // NOTE: On ne peut pas comparer le montant car events.rs est vide.
    Ok(())
}