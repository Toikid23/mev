// DANS : src/execution/transaction_builder.rs

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
// MODIFIÉ : On retire Mutex car il n'est plus utilisé ici.
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
    graph: Arc<Graph>, // MODIFIÉ : On reçoit le nouveau Arc<Graph>
    rpc_client: &ResilientRpcClient,
    payer: &Keypair,
    lookup_table_account: &AddressLookupTableAccount,
    protections: Option<&SwapProtections>,
) -> Result<(VersionedTransaction, Instruction)> {
    println!("\n--- [Phase 1] Construction de la Transaction V0 (avec LUT) ---");

    // --- LA CORRECTION EST ICI ---
    // On remplace le verrou Mutex par des verrous fins RwLock en lecture.
    let (pool_buy_from, pool_sell_to) = {
        // 1. On prend un verrou en lecture sur la map des pools
        let pools_reader = graph.pools.read().await;

        // 2. On récupère les Arcs vers les pools individuels
        let buy_from_arc_rwlock = pools_reader.get(&opportunity.pool_buy_from_key).unwrap().clone();
        let sell_to_arc_rwlock = pools_reader.get(&opportunity.pool_sell_to_key).unwrap().clone();

        // On peut relâcher le verrou de la map maintenant.
        drop(pools_reader);

        // 3. On prend les verrous en lecture sur les pools individuels et on clone leurs données.
        let (buy_guard, sell_guard) = tokio::join!(buy_from_arc_rwlock.read(), sell_to_arc_rwlock.read());
        (buy_guard.clone(), sell_guard.clone())
    }; // Tous les verrous sont relâchés ici.
    // --- FIN DE LA CORRECTION ---

    // Le reste de la fonction est absolument identique.
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
        (1, 1)
    };

    let ix_buy = pool_buy_from.create_swap_instruction(
        &opportunity.token_in_mint,
        opportunity.amount_in,
        min_out_buy,
        &user_accounts_buy
    )?;
    let ix_sell = pool_sell_to.create_swap_instruction(
        &opportunity.token_intermediate_mint,
        predicted_intermediate_out,
        min_out_sell,
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
        minimum_expected_profit: final_profit_protection,
    };
    let mut instruction_data = Vec::new();
    instruction_data.extend_from_slice(&[246, 14, 81, 121, 140, 237, 86, 23]);
    instruction_data.extend_from_slice(&args.try_to_vec()?);

    let (config_pda, _) = Pubkey::find_program_address(&[b"config"], &ATOMIC_ARB_EXECUTOR_PROGRAM_ID);
    let mut final_accounts_list = vec![
        AccountMeta::new_readonly(config_pda, false),
        AccountMeta::new(payer.pubkey(), true),
        // --- LA CORRECTION EST ICI ---
        // On construit la struct directement pour pouvoir spécifier `is_writable`.
        AccountMeta {
            pubkey: user_accounts_sell.destination,
            is_signer: false,
            is_writable: true,
        },
    ];
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