use anyhow::{Result};
use solana_sdk::message::AddressLookupTableAccount;
use solana_sdk::{
    instruction::{Instruction, AccountMeta},
    pubkey::Pubkey,
    signature::Keypair,
    signer::Signer,
    transaction::VersionedTransaction,
    message::{v0, VersionedMessage},
};
use solana_sdk::compute_budget::ComputeBudgetInstruction;
use spl_associated_token_account::get_associated_token_address;
use anchor_lang::{prelude::*, AnchorSerialize};
use std::sync::Arc;
use crate::execution::protections::SwapProtections;

use crate::{
    decoders::{PoolOperations},
    graph_engine::Graph, // On importe bien notre nouvelle structure Graph
    strategies::spatial::ArbitrageOpportunity,
    decoders::pool_operations::UserSwapAccounts,
    rpc::ResilientRpcClient,
};

// NOTE: Ces constantes doivent être accessibles ici.
const ATOMIC_ARB_EXECUTOR_PROGRAM_ID: Pubkey = solana_sdk::pubkey!("3gHUHkQD8TjeQntEsygDnm4TRo3xKQRTbDTaFxgQdXe1");

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
    graph: Arc<Graph>,
    rpc_client: &ResilientRpcClient,
    payer: &Keypair,
    lookup_table_account: &AddressLookupTableAccount,
    protections: &SwapProtections, // On le rend obligatoire
    estimated_cus: u64,
    priority_fee_price_per_cu: u64,
) -> Result<(VersionedTransaction, Instruction)> {
    println!("\n--- [Phase 2] Construction de la Transaction V0 Finale ---");

    let pool_buy_from = graph.pools.get(&opportunity.pool_buy_from_key).unwrap().clone();
    let pool_sell_to = graph.pools.get(&opportunity.pool_sell_to_key).unwrap().clone();

    // On doit recalculer la sortie intermédiaire attendue pour l'instruction de vente
    let quote_result_buy = pool_buy_from.get_quote_with_details(
        &opportunity.token_in_mint,
        opportunity.amount_in,
        0,
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

    let ix_buy = pool_buy_from.create_swap_instruction(
        &opportunity.token_in_mint,
        opportunity.amount_in,
        protections.intermediate_min_amount_out, // Utilise la protection
        &user_accounts_buy,
    )?;
    let ix_sell = pool_sell_to.create_swap_instruction(
        &opportunity.token_intermediate_mint,
        quote_result_buy.amount_out, // On utilise la sortie prédite comme entrée pour le 2ème swap
        protections.final_min_amount_out, // Utilise la protection
        &user_accounts_sell,
    )?;

    // ... (la logique de construction de `execute_route_ix` reste la même)
    let step1 = ProgramSwapStep { dex_program_id: ix_buy.program_id, num_accounts_for_step: ix_buy.accounts.len() as u8, instruction_data: ix_buy.data };
    let step2 = ProgramSwapStep { dex_program_id: ix_sell.program_id, num_accounts_for_step: ix_sell.accounts.len() as u8, instruction_data: ix_sell.data };
    let expected_return = opportunity.amount_in.saturating_add(opportunity.profit_in_lamports);
    let final_profit_protection = expected_return.saturating_sub(protections.final_min_amount_out);
    let args = ExecuteRouteIxArgs { route: vec![step1, step2], minimum_expected_profit: final_profit_protection };
    let mut instruction_data = Vec::new();
    instruction_data.extend_from_slice(&[246, 14, 81, 121, 140, 237, 86, 23]);
    instruction_data.extend_from_slice(&args.try_to_vec()?);
    let (config_pda, _) = Pubkey::find_program_address(&[b"config"], &ATOMIC_ARB_EXECUTOR_PROGRAM_ID);
    let mut final_accounts_list = vec![
        AccountMeta::new_readonly(config_pda, false),
        AccountMeta::new(payer.pubkey(), true),
        AccountMeta { pubkey: user_accounts_sell.destination, is_signer: false, is_writable: true },
    ];
    final_accounts_list.extend(ix_buy.accounts);
    final_accounts_list.extend(ix_sell.accounts);
    let execute_route_ix = Instruction { program_id: ATOMIC_ARB_EXECUTOR_PROGRAM_ID, accounts: final_accounts_list, data: instruction_data };


    // <-- NOUVEAU : On injecte les instructions de Compute Budget
    let set_compute_unit_limit_ix = ComputeBudgetInstruction::set_compute_unit_limit(estimated_cus as u32);
    let set_compute_unit_price_ix = ComputeBudgetInstruction::set_compute_unit_price(priority_fee_price_per_cu);

    let all_instructions = vec![
        set_compute_unit_limit_ix,
        set_compute_unit_price_ix,
        execute_route_ix.clone(), // On clone l'instruction de base pour la retourner
    ];

    let recent_blockhash = rpc_client.get_latest_blockhash().await?;
    let message = v0::Message::try_compile(
        &payer.pubkey(),
        &all_instructions, // On utilise le vecteur complet
        &[lookup_table_account.clone()],
        recent_blockhash,
    )?;
    let transaction = VersionedTransaction::try_new(VersionedMessage::V0(message), &[payer])?;

    println!("  -> Transaction V0 Finale construite avec CUs et frais de priorité.");
    Ok((transaction, execute_route_ix))
}