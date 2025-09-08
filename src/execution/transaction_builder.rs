use anyhow::Result;
use solana_sdk::message::AddressLookupTableAccount;
use solana_sdk::{
    instruction::{Instruction, AccountMeta},
    pubkey::Pubkey,
    signature::Keypair,
    signer::Signer,
    transaction::VersionedTransaction,
    message::{v0, VersionedMessage},
};
use spl_associated_token_account::get_associated_token_address;
use anchor_lang::{prelude::*, AnchorSerialize};
use tokio::sync::Mutex;
use std::sync::Arc;
use crate::execution::protections::SwapProtections;

use crate::{
    decoders::{PoolOperations},
    graph_engine::Graph,
    strategies::spatial::ArbitrageOpportunity,
    decoders::pool_operations::UserSwapAccounts,
    rpc::ResilientRpcClient,
};

// NOTE: Ces constantes doivent être accessibles ici.
const ATOMIC_ARB_EXECUTOR_PROGRAM_ID: Pubkey = pubkey!("3gHUHkQD8TjeQntEsygDnm4TRo3xKQRTbDTaFxgQdXe1");

#[derive(AnchorSerialize, Clone, Debug)]
pub struct ProgramSwapStep {
    pub dex_program_id: Pubkey,
    pub num_accounts_for_step: u8,
    pub instruction_data: Vec<u8>,
}

#[derive(AnchorSerialize)]
pub struct ExecuteRouteIxArgs {
    pub route: Vec<ProgramSwapStep>,
    pub minimum_expected_profit: u64,
}

// C'est votre fonction originale, maintenant dans son propre module.
pub async fn build_arbitrage_transaction(
    opportunity: &ArbitrageOpportunity,
    graph: Arc<Mutex<Graph>>,
    rpc_client: &ResilientRpcClient,
    payer: &Keypair,
    lookup_table_account: &AddressLookupTableAccount,
    protections: Option<&SwapProtections>,
) -> Result<(VersionedTransaction, Instruction)> {
    println!("\n--- [Phase 1] Construction de la Transaction V0 (avec LUT) ---");

    let graph_guard = graph.lock().await;
    let pool_buy_from = graph_guard.pools.get(&opportunity.pool_buy_from_key).unwrap().clone();
    let pool_sell_to = graph_guard.pools.get(&opportunity.pool_sell_to_key).unwrap().clone();
    drop(graph_guard);

    let predicted_intermediate_out = pool_buy_from.get_quote(
        &opportunity.token_in_mint,
        opportunity.amount_in,
        0, // Timestamp (0 est acceptable pour cette estimation)
    )?;

    let user_accounts_buy = UserSwapAccounts {
        owner: payer.pubkey(),
        source: get_associated_token_address(&payer.pubkey(), &opportunity.token_in_mint),
        destination: get_associated_token_address(&payer.pubkey(), &opportunity.token_intermediate_mint),
    };
    let user_accounts_sell = UserSwapAccounts {
        owner: payer.pubkey(),
        source: get_associated_token_address(&payer.pubkey(), &opportunity.token_intermediate_mint),
        destination: get_associated_token_address(&payer.pubkey(), &opportunity.token_in_mint),
    };

    let (min_out_buy, min_out_sell) = if let Some(p) = protections {
        (p.intermediate_min_amount_out, p.final_min_amount_out)
    } else {
        // Pas de protections fournies, on est en mode "simulation de découverte".
        (1, 1)
    };

    let ix_buy = pool_buy_from.create_swap_instruction(
        &opportunity.token_in_mint,
        opportunity.amount_in,
        min_out_buy, // <-- UTILISER LA VARIABLE
        &user_accounts_buy
    )?;
    let ix_sell = pool_sell_to.create_swap_instruction(
        &opportunity.token_intermediate_mint,
        predicted_intermediate_out,
        min_out_sell, // <-- UTILISER LA VARIABLE
        &user_accounts_sell
    )?;

    let step1 = ProgramSwapStep {
        dex_program_id: ix_buy.program_id,
        num_accounts_for_step: ix_buy.accounts.len() as u8,
        instruction_data: ix_buy.data,
    };
    let step2 = ProgramSwapStep {
        dex_program_id: ix_sell.program_id,
        num_accounts_for_step: ix_sell.accounts.len() as u8,
        instruction_data: ix_sell.data,
    };
    let final_profit_protection = if let Some(p) = protections {
        let expected_return = opportunity.amount_in.saturating_add(opportunity.profit_in_lamports);
        expected_return.saturating_sub(p.final_min_amount_out)
    } else {
        0
    };

    let args = ExecuteRouteIxArgs {
        route: vec![step1, step2],
        minimum_expected_profit: final_profit_protection, // <-- UTILISER LA VARIABLE
    };
    let mut instruction_data = Vec::new();
    instruction_data.extend_from_slice(&[246, 14, 81, 121, 140, 237, 86, 23]);
    instruction_data.extend_from_slice(&args.try_to_vec()?);

    let (config_pda, _) = Pubkey::find_program_address(&[b"config"], &ATOMIC_ARB_EXECUTOR_PROGRAM_ID);
    let mut final_accounts_list = vec![
        AccountMeta::new_readonly(config_pda, false),
        AccountMeta::new(payer.pubkey(), true),
    ];
    final_accounts_list.push(AccountMeta { pubkey: user_accounts_sell.destination, is_signer: false, is_writable: true });
    final_accounts_list.extend(ix_buy.accounts);
    final_accounts_list.extend(ix_sell.accounts);

    let execute_route_ix = Instruction {
        program_id: ATOMIC_ARB_EXECUTOR_PROGRAM_ID,
        accounts: final_accounts_list,
        data: instruction_data,
    };

    let recent_blockhash = rpc_client.get_latest_blockhash().await?;

    let message = v0::Message::try_compile(
        &payer.pubkey(),
        &[execute_route_ix.clone()],
        &[lookup_table_account.clone()],
        recent_blockhash,
    )?;

    let transaction = VersionedTransaction::try_new(VersionedMessage::V0(message), &[payer])?;

    println!("  -> Transaction V0 construite avec succès.");
    Ok((transaction, execute_route_ix))
}