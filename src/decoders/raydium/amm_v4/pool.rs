// DANS: src/decoders/raydium_decoders/pool
// VERSION AVEC create_swap_instruction DANS L'IMPL


use bytemuck::{from_bytes, Pod, Zeroable, cast};
use solana_sdk::pubkey::Pubkey;
use anyhow::{bail, Result, anyhow};
use solana_client::nonblocking::rpc_client::RpcClient;
use crate::decoders::spl_token_decoders;
use solana_sdk::{instruction::{AccountMeta, Instruction}, pubkey};
use crate::decoders::raydium::amm_v4::openbook_market::{OrderBook};
use openbook_dex::state::MarketState;
use std::mem::size_of;
use serde::{Serialize, Deserialize};
use async_trait::async_trait;

use crate::decoders::pool_operations::{PoolOperations, UserSwapAccounts};


// La structure de données reste la même
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecodedAmmPool {
    pub address: Pubkey, pub nonce: u64, pub mint_a: Pubkey, pub mint_b: Pubkey,
    pub vault_a: Pubkey, pub vault_b: Pubkey, pub market: Pubkey,
    pub market_program_id: Pubkey, pub market_bids: Pubkey, pub market_asks: Pubkey,
    pub market_event_queue: Pubkey, pub market_coin_vault: Pubkey, pub market_pc_vault: Pubkey,
    pub market_vault_signer: Pubkey, pub open_orders: Pubkey, pub target_orders: Pubkey,
    pub mint_a_decimals: u8, pub mint_b_decimals: u8, pub mint_a_transfer_fee_bps: u16,
    pub mint_b_transfer_fee_bps: u16, pub trade_fee_numerator: u64, pub trade_fee_denominator: u64,
    pub reserve_a: u64, pub reserve_b: u64,
    pub bids_order_book: Option<OrderBook>, pub asks_order_book: Option<OrderBook>,
    pub coin_lot_size: u64, pub pc_lot_size: u64,
    pub last_swap_timestamp: i64,
}

// Les structures on-chain restent les mêmes
#[repr(C, packed)] #[derive(Clone, Copy, Pod, Zeroable, Debug)] struct Fees { pub min_separate_numerator: u64, pub min_separate_denominator: u64, pub trade_fee_numerator: u64, pub trade_fee_denominator: u64, pub pnl_numerator: u64, pub pnl_denominator: u64, pub swap_fee_numerator: u64, pub swap_fee_denominator: u64, }
#[repr(C, packed)] #[derive(Clone, Copy, Pod, Zeroable, Debug)] struct OutPutData { pub need_take_pnl_coin: u64, pub need_take_pnl_pc: u64, pub total_pnl_pc: u64, pub total_pnl_coin: u64, pub pool_open_time: u64, pub punish_pc_amount: u64, pub punish_coin_amount: u64, pub orderbook_to_init_time: u64, pub swap_coin_in_amount: u128, pub swap_pc_out_amount: u128, pub swap_take_pc_fee: u64, pub swap_pc_in_amount: u128, pub swap_coin_out_amount: u128, pub swap_take_coin_fee: u64, }
#[repr(C, packed)] #[derive(Clone, Copy, Pod, Zeroable, Debug)] struct AmmInfoData { pub status: u64, pub nonce: u64, pub order_num: u64, pub depth: u64, pub coin_decimals: u64, pub pc_decimals: u64, pub state: u64, pub reset_flag: u64, pub min_size: u64, pub vol_max_cut_ratio: u64, pub amount_wave: u64, pub coin_lot_size: u64, pub pc_lot_size: u64, pub min_price_multiplier: u64, pub max_price_multiplier: u64, pub sys_decimal_value: u64, pub fees: Fees, pub out_put: OutPutData, pub token_coin: Pubkey, pub token_pc: Pubkey, pub coin_mint: Pubkey, pub pc_mint: Pubkey, pub lp_mint: Pubkey, pub open_orders: Pubkey, pub market: Pubkey, pub serum_dex: Pubkey, pub target_orders: Pubkey, pub withdraw_queue: Pubkey, pub token_temp_lp: Pubkey, pub amm_owner: Pubkey, pub lp_amount: u64, pub client_order_id: u64, pub padding: [u64; 2], }

//ADDRESS DEVNET !!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!
//pub const RAYDIUM_AMM_V4_PROGRAM_ID: Pubkey = pubkey!("DRaya7Kj3aMWQSy19kSjvmuwq9docCHofyP9kanQGaav");

pub const RAYDIUM_AMM_V4_PROGRAM_ID: Pubkey = pubkey!("675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8");

// --- DÉBUT DE LA MODIFICATION ---
impl DecodedAmmPool {
    pub fn fee_as_percent(&self) -> f64 {
        if self.trade_fee_denominator == 0 { return 0.0; }
        (self.trade_fee_numerator as f64 / self.trade_fee_denominator as f64) * 100.0
    }


}
// --- FIN DE LA MODIFICATION ---

// Les fonctions decode_pool, hydrate, et l'implémentation de PoolOperations restent inchangées.
pub fn decode_pool(address: &Pubkey, data: &[u8]) -> Result<DecodedAmmPool> {
    if data.len() < size_of::<AmmInfoData>() { bail!("AMM V4 data length mismatch."); }
    let pool_struct: &AmmInfoData = from_bytes(&data[..size_of::<AmmInfoData>()]);
    if pool_struct.status == 0 { bail!("Pool {} is not initialized.", address); }

    Ok(DecodedAmmPool {
        address: *address, nonce: pool_struct.nonce, market: pool_struct.market,
        market_program_id: pool_struct.serum_dex, open_orders: pool_struct.open_orders,
        target_orders: pool_struct.target_orders, mint_a: pool_struct.coin_mint,
        mint_b: pool_struct.pc_mint, vault_a: pool_struct.token_coin,
        vault_b: pool_struct.token_pc, trade_fee_numerator: pool_struct.fees.trade_fee_numerator,
        trade_fee_denominator: pool_struct.fees.trade_fee_denominator, mint_a_decimals: 0,
        mint_b_decimals: 0, mint_a_transfer_fee_bps: 0,
        mint_b_transfer_fee_bps: 0, reserve_a: 0, reserve_b: 0, market_bids: Pubkey::default(),
        market_asks: Pubkey::default(), market_event_queue: Pubkey::default(),
        market_coin_vault: Pubkey::default(), market_pc_vault: Pubkey::default(),
        market_vault_signer: Pubkey::default(), bids_order_book: None,
        asks_order_book: None, coin_lot_size: 0, pc_lot_size: 0,
        last_swap_timestamp: 0,
    })
}

pub async fn hydrate(pool: &mut DecodedAmmPool, rpc_client: &RpcClient) -> Result<()> {
    let accounts_to_fetch = vec![pool.vault_a, pool.vault_b, pool.mint_a, pool.mint_b, pool.market];
    let mut accounts = rpc_client.get_multiple_accounts(&accounts_to_fetch).await?;

    let vault_a_account = accounts[0].take().ok_or_else(|| anyhow!("Vault A non trouvé"))?;
    pool.reserve_a = u64::from_le_bytes(vault_a_account.data[64..72].try_into()?);
    let vault_b_account = accounts[1].take().ok_or_else(|| anyhow!("Vault B non trouvé"))?;
    pool.reserve_b = u64::from_le_bytes(vault_b_account.data[64..72].try_into()?);

    let mint_a_data = accounts[2].take().ok_or_else(|| anyhow!("Mint A non trouvé"))?;
    let decoded_mint_a = spl_token_decoders::mint::decode_mint(&pool.mint_a, &mint_a_data.data)?;
    pool.mint_a_decimals = decoded_mint_a.decimals;
    pool.mint_a_transfer_fee_bps = decoded_mint_a.transfer_fee_basis_points;

    let mint_b_data = accounts[3].take().ok_or_else(|| anyhow!("Mint B non trouvé"))?;
    let decoded_mint_b = spl_token_decoders::mint::decode_mint(&pool.mint_b, &mint_b_data.data)?;
    pool.mint_b_decimals = decoded_mint_b.decimals;
    pool.mint_b_transfer_fee_bps = decoded_mint_b.transfer_fee_basis_points;

    let market_account = accounts[4].take().ok_or_else(|| anyhow!("Market non trouvé"))?;
    let market_data = &market_account.data;
    let market_state: &MarketState = from_bytes(
        market_data.get(5..5 + size_of::<MarketState>())
            .ok_or_else(|| anyhow!("Données du marché invalides"))?
    );
    pool.market_bids = cast(market_state.bids);
    pool.market_asks = cast(market_state.asks);
    pool.market_event_queue = cast(market_state.event_q);
    pool.market_coin_vault = cast(market_state.coin_vault);
    pool.market_pc_vault = cast(market_state.pc_vault);
    let vault_signer_nonce = market_state.vault_signer_nonce;
    pool.market_vault_signer = Pubkey::create_program_address(
        &[&pool.market.to_bytes(), &vault_signer_nonce.to_le_bytes()],
        &pool.market_program_id,
    )?;

    Ok(())
}
#[async_trait]
impl PoolOperations for DecodedAmmPool {
    fn get_mints(&self) -> (Pubkey, Pubkey) { (self.mint_a, self.mint_b) }
    fn get_vaults(&self) -> (Pubkey, Pubkey) { (self.vault_a, self.vault_b) }
    fn address(&self) -> Pubkey { self.address }

    fn get_quote(&self, token_in_mint: &Pubkey, amount_in: u64, _current_timestamp: i64) -> Result<u64> {
        // Les logs ne sont plus nécessaires car l'approche change
        let (in_mint_fee_bps, out_mint_fee_bps, in_reserve, out_reserve) = if *token_in_mint == self.mint_a {
            (self.mint_a_transfer_fee_bps, self.mint_b_transfer_fee_bps, self.reserve_a, self.reserve_b)
        } else {
            (self.mint_b_transfer_fee_bps, self.mint_a_transfer_fee_bps, self.reserve_b, self.reserve_a)
        };
        if in_reserve == 0 || out_reserve == 0 { return Ok(0); }

        // Étape 1: Frais de transfert sur l'input (inchangé)
        let fee_on_input = (amount_in as u128 * in_mint_fee_bps as u128) / 10000;
        let amount_in_after_transfer_fee = amount_in.saturating_sub(fee_on_input as u64) as u128;

        // Étape 2: Calcul du swap brut, SANS les frais de pool
        let denominator = (in_reserve as u128).saturating_add(amount_in_after_transfer_fee);
        if denominator == 0 { return Ok(0); }
        let gross_amount_out = amount_in_after_transfer_fee.saturating_mul(out_reserve as u128) / denominator;

        // Étape 3: Appliquer les frais de pool sur le montant de sortie brut
        let swap_fee_numerator = 25;
        let swap_fee_denominator = 10000;
        let fee_amount = (gross_amount_out * swap_fee_numerator) / swap_fee_denominator; // Floor division
        let amount_out_after_pool_fee = gross_amount_out.saturating_sub(fee_amount);

        // Étape 4: Frais de transfert sur l'output (inchangé)
        let fee_on_output = (amount_out_after_pool_fee * out_mint_fee_bps as u128) / 10000;
        let final_amount_out = amount_out_after_pool_fee.saturating_sub(fee_on_output);

        Ok(final_amount_out as u64)
    }

    fn get_required_input(
        &self,
        token_out_mint: &Pubkey,
        amount_out: u64,
        _current_timestamp: i64
    ) -> Result<u64> {
        if amount_out == 0 { return Ok(0); }

        let (in_mint_fee_bps, out_mint_fee_bps, in_reserve, out_reserve) = if *token_out_mint == self.mint_b {
            (self.mint_a_transfer_fee_bps, self.mint_b_transfer_fee_bps, self.reserve_a, self.reserve_b)
        } else {
            (self.mint_b_transfer_fee_bps, self.mint_a_transfer_fee_bps, self.reserve_b, self.reserve_a)
        };

        if out_reserve == 0 || in_reserve == 0 { return Err(anyhow!("Pool has no liquidity")); }

        const BPS_DENOMINATOR: u128 = 10000;

        // Étape 1: Inverser les frais de transfert sur l'output
        let amount_out_after_pool_fee = if out_mint_fee_bps > 0 {
            let num = (amount_out as u128).saturating_mul(BPS_DENOMINATOR);
            let den = BPS_DENOMINATOR.saturating_sub(out_mint_fee_bps as u128);
            num.div_ceil(den)
        } else {
            amount_out as u128
        };

        // Étape 2: Inverser les frais de pool
        let swap_fee_numerator = 25;
        let swap_fee_denominator = 10000;
        let gross_amount_out = if swap_fee_numerator > 0 {
            let num = amount_out_after_pool_fee.saturating_mul(swap_fee_denominator);
            let den = swap_fee_denominator.saturating_sub(swap_fee_numerator);
            num.div_ceil(den)
        } else {
            amount_out_after_pool_fee
        };

        if gross_amount_out >= out_reserve as u128 {
            return Err(anyhow!("Cannot get required input, amount_out is too high."));
        }

        // Étape 3: Inverser la formule de swap
        let numerator_xy = gross_amount_out.saturating_mul(in_reserve as u128);
        let denominator_xy = (out_reserve as u128).saturating_sub(gross_amount_out);
        let amount_in_after_transfer_fee = numerator_xy.div_ceil(denominator_xy);

        // Étape 4: Inverser les frais de transfert sur l'input
        let required_amount_in = if in_mint_fee_bps > 0 {
            let num = amount_in_after_transfer_fee.saturating_mul(BPS_DENOMINATOR);
            let den = BPS_DENOMINATOR.saturating_sub(in_mint_fee_bps as u128);
            num.div_ceil(den)
        } else {
            amount_in_after_transfer_fee
        };

        Ok(required_amount_in as u64)
    }

    async fn get_quote_async(&mut self, token_in_mint: &Pubkey, amount_in: u64, _rpc_client: &RpcClient) -> Result<u64> {
        self.get_quote(token_in_mint, amount_in, 0)
    }

    // --- VOTRE create_swap_instruction ORIGINAL, VALIDÉ ET CORRECT ---
    fn create_swap_instruction(
        &self,
        _token_in_mint: &Pubkey,
        amount_in: u64,
        minimum_amount_out: u64,
        user_accounts: &UserSwapAccounts,
    ) -> Result<Instruction> {
        let mut instruction_data = vec![9];
        instruction_data.extend_from_slice(&amount_in.to_le_bytes());
        instruction_data.extend_from_slice(&minimum_amount_out.to_le_bytes());

        let (amm_authority, _) = Pubkey::find_program_address(
            &[b"amm authority"],
            &RAYDIUM_AMM_V4_PROGRAM_ID,
        );

        let keys = vec![
            // NOTE: La ligne incorrecte a été supprimée. La liste commence par spl_token::id().
            AccountMeta::new_readonly(spl_token::id(), false),
            AccountMeta::new(self.address, false),
            AccountMeta::new_readonly(amm_authority, false),
            AccountMeta::new(self.open_orders, false),
            AccountMeta::new(self.target_orders, false),
            AccountMeta::new(self.vault_a, false),
            AccountMeta::new(self.vault_b, false),
            AccountMeta::new_readonly(self.market_program_id, false),
            AccountMeta::new(self.market, false),
            AccountMeta::new(self.market_bids, false),
            AccountMeta::new(self.market_asks, false),
            AccountMeta::new(self.market_event_queue, false),
            AccountMeta::new(self.market_coin_vault, false),
            AccountMeta::new(self.market_pc_vault, false),
            AccountMeta::new_readonly(self.market_vault_signer, false),
            AccountMeta::new(user_accounts.source, false),
            AccountMeta::new(user_accounts.destination, false),
            AccountMeta::new_readonly(user_accounts.owner, true),
        ];

        Ok(Instruction {
            program_id: RAYDIUM_AMM_V4_PROGRAM_ID,
            accounts: keys,
            data: instruction_data,
        })
    }
}