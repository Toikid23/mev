// DANS : src/execution/transaction_builder.rs (VERSION NETTOYÉE ET FINALE)

use anyhow::{Result, anyhow};
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

use crate::{
    decoders::{PoolOperations},
    graph_engine::Graph,
    strategies::spatial::ArbitrageOpportunity,
    decoders::pool_operations::UserSwapAccounts,
    rpc::ResilientRpcClient,
    execution::protections::SwapProtections,
};

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

#[derive(Clone)] // On ajoute Clone pour pouvoir le stocker dans le cache
pub struct ArbitrageInstructionsTemplate {
    pub ix_buy: Instruction,
    pub ix_sell: Instruction,
    pub ix_execute: Instruction,
    pub buy_amount_in_offset: usize,
    pub buy_min_amount_out_offset: usize,
    pub sell_amount_in_offset: usize,
    pub sell_min_amount_out_offset: usize,
}

impl ArbitrageInstructionsTemplate {
    fn find_amount_offsets(ix_data: &[u8]) -> Result<(usize, usize)> {
        if ix_data.is_empty() { return Err(anyhow!("Instruction data is empty")); }
        let discriminator = &ix_data[0..std::cmp::min(8, ix_data.len())];

        match discriminator {
            [143, 190, 90, 218, 196, 30, 51, 222] => Ok((8, 16)), // Raydium CPMM Sell
            [43, 4, 237, 11, 26, 201, 30, 98] => Ok((8, 16)), // Raydium CLMM
            [248, 198, 158, 145, 225, 117, 135, 200] => Ok((8, 16)), // Orca & Meteora DAMM
            [65, 75, 63, 76, 235, 91, 91, 136] => Ok((8, 16)), // Meteora DLMM
            [102, 6, 61, 18, 1, 218, 235, 234] => Ok((16, 8)), // Pump.fun Buy
            [51, 230, 133, 164, 1, 127, 131, 173] => Ok((8, 16)), // Pump.fun Sell
            _ => {
                if ix_data[0] == 9 {
                    Ok((1, 9)) // Raydium AMMv4
                } else {
                    Err(anyhow!("Discriminateur d'instruction de swap inconnu: {:?}", discriminator))
                }
            }
        }
    }

    pub fn new(opportunity: &ArbitrageOpportunity, graph: &Graph, payer_pubkey: &Pubkey) -> Result<Self> {
        let pool_buy_from = graph.pools.get(&opportunity.pool_buy_from_key).ok_or_else(|| anyhow!("Pool buy_from non trouvé"))?;
        let pool_sell_to = graph.pools.get(&opportunity.pool_sell_to_key).ok_or_else(|| anyhow!("Pool sell_to non trouvé"))?;

        let user_accounts_buy = UserSwapAccounts {
            owner: *payer_pubkey,
            source: get_associated_token_address(payer_pubkey, &opportunity.token_in_mint),
            destination: get_associated_token_address(payer_pubkey, &opportunity.token_intermediate_mint),
        };
        let user_accounts_sell = UserSwapAccounts {
            owner: *payer_pubkey,
            source: get_associated_token_address(payer_pubkey, &opportunity.token_intermediate_mint),
            destination: get_associated_token_address(payer_pubkey, &opportunity.token_in_mint),
        };

        let ix_buy = pool_buy_from.create_swap_instruction(&opportunity.token_in_mint, 0, 0, &user_accounts_buy)?;
        let ix_sell = pool_sell_to.create_swap_instruction(&opportunity.token_intermediate_mint, 0, 0, &user_accounts_sell)?;

        let (buy_amount_in_offset, buy_min_amount_out_offset) = Self::find_amount_offsets(&ix_buy.data)?;
        let (sell_amount_in_offset, sell_min_amount_out_offset) = Self::find_amount_offsets(&ix_sell.data)?;

        let (config_pda, _) = Pubkey::find_program_address(&[b"config"], &ATOMIC_ARB_EXECUTOR_PROGRAM_ID);
        let mut final_accounts_list = vec![
            AccountMeta::new_readonly(config_pda, false),
            AccountMeta::new(*payer_pubkey, true),
            AccountMeta { pubkey: user_accounts_sell.destination, is_signer: false, is_writable: true },
        ];
        final_accounts_list.extend_from_slice(&ix_buy.accounts);
        final_accounts_list.extend_from_slice(&ix_sell.accounts);

        let ix_execute = Instruction {
            program_id: ATOMIC_ARB_EXECUTOR_PROGRAM_ID,
            accounts: final_accounts_list,
            data: vec![],
        };

        Ok(Self {
            ix_buy,
            ix_sell,
            ix_execute,
            buy_amount_in_offset,
            buy_min_amount_out_offset,
            sell_amount_in_offset,
            sell_min_amount_out_offset,
        })
    }
}

pub async fn build_from_template(
    template: &ArbitrageInstructionsTemplate,
    amount_in: u64,
    intermediate_amount_out: u64,
    protections: &SwapProtections,
    estimated_cus: u64,
    priority_fee_price_per_cu: u64,
    rpc_client: &ResilientRpcClient,
    payer: &Keypair,
    lookup_table_account: &AddressLookupTableAccount,
) -> Result<VersionedTransaction> {

    let mut ix_buy = template.ix_buy.clone();
    let mut ix_sell = template.ix_sell.clone();
    let mut ix_execute = template.ix_execute.clone();

    ix_buy.data[template.buy_amount_in_offset..template.buy_amount_in_offset + 8].copy_from_slice(&amount_in.to_le_bytes());
    ix_buy.data[template.buy_min_amount_out_offset..template.buy_min_amount_out_offset + 8].copy_from_slice(&protections.intermediate_min_amount_out.to_le_bytes());

    ix_sell.data[template.sell_amount_in_offset..template.sell_amount_in_offset + 8].copy_from_slice(&intermediate_amount_out.to_le_bytes());
    ix_sell.data[template.sell_min_amount_out_offset..template.sell_min_amount_out_offset + 8].copy_from_slice(&protections.final_min_amount_out.to_le_bytes());

    let step1 = ProgramSwapStep { dex_program_id: ix_buy.program_id, num_accounts_for_step: ix_buy.accounts.len() as u8, instruction_data: ix_buy.data };
    let step2 = ProgramSwapStep { dex_program_id: ix_sell.program_id, num_accounts_for_step: ix_sell.accounts.len() as u8, instruction_data: ix_sell.data };

    let min_profit = protections.final_min_amount_out.saturating_sub(amount_in);
    let args = ExecuteRouteIxArgs { route: vec![step1, step2], minimum_expected_profit: min_profit };

    let mut execute_data = Vec::with_capacity(512);
    execute_data.extend_from_slice(&[246, 14, 81, 121, 140, 237, 86, 23]);
    execute_data.extend_from_slice(&args.try_to_vec()?);
    ix_execute.data = execute_data;

    let set_compute_unit_limit_ix = ComputeBudgetInstruction::set_compute_unit_limit(estimated_cus as u32);
    let set_compute_unit_price_ix = ComputeBudgetInstruction::set_compute_unit_price(priority_fee_price_per_cu);

    let all_instructions = vec![
        set_compute_unit_limit_ix,
        set_compute_unit_price_ix,
        ix_execute,
    ];

    let recent_blockhash = rpc_client.get_latest_blockhash().await?;
    let message = v0::Message::try_compile(
        &payer.pubkey(),
        &all_instructions,
        std::slice::from_ref(lookup_table_account),
        recent_blockhash,
    )?;

    Ok(VersionedTransaction::try_new(VersionedMessage::V0(message), &[payer])?)
}