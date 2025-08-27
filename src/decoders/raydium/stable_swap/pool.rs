// src/decoders/raydium_decoders/pool

use crate::decoders::pool_operations::PoolOperations;
use bytemuck::{from_bytes, Pod, Zeroable};
use solana_sdk::pubkey::Pubkey;
use anyhow::{bail, Result, anyhow};
use super::math;
use serde::{Serialize, Deserialize};
use async_trait::async_trait;
use solana_client::nonblocking::rpc_client::RpcClient;


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecodedStableSwapPool {
    pub address: Pubkey,
    pub mint_a: Pubkey,
    pub mint_b: Pubkey,
    pub vault_a: Pubkey,
    pub vault_b: Pubkey,
    pub model_data_account: Pubkey,
    pub total_fee_percent: f64,
    // Le paramètre mathématique clé
    pub amp: u64,
    // Les réserves seront hydratées plus tard
    pub reserve_a: u64,
    pub reserve_b: u64,
    pub last_swap_timestamp: i64,
}


// --- STRUCTURES BRUTES (Traduction de stable_stats.rs) ---

#[repr(C, packed)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
pub struct Fees {
    pub min_separate_numerator: u64,
    pub min_separate_denominator: u64,
    pub trade_fee_numerator: u64,
    pub trade_fee_denominator: u64,
    pub pnl_numerator: u64,
    pub pnl_denominator: u64,
    pub swap_fee_numerator: u64,
    pub swap_fee_denominator: u64,
}

#[repr(C, packed)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
pub struct OutPutData {
    pub need_take_pnl_coin: u64, pub need_take_pnl_pc: u64,
    pub total_pnl_pc: u64, pub total_pnl_coin: u64,
    pub pool_open_time: u64, pub punish_pc_amount: u64,
    pub punish_coin_amount: u64, pub orderbook_to_init_time: u64,
    pub swap_coin_in_amount: u128, pub swap_pc_out_amount: u128,
    pub swap_pc_in_amount: u128, pub swap_coin_out_amount: u128,
    pub swap_pc_fee: u64, pub swap_coin_fee: u64,
}

#[repr(C, packed)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
pub struct StableSwapAmmInfoData {
    pub account_type: u64, pub status: u64, pub nonce: u64,
    pub order_num: u64, pub depth: u64, pub coin_decimals: u64,
    pub pc_decimals: u64, pub state: u64, pub reset_flag: u64,
    pub min_size: u64, pub vol_max_cut_ratio: u64, pub amount_wave: u64,
    pub coin_lot_size: u64, pub pc_lot_size: u64,
    pub min_price_multiplier: u64, pub max_price_multiplier: u64,
    pub sys_decimal_value: u64, pub abort_trade_factor: u64,
    pub price_tick_multiplier: u64, pub price_tick: u64,
    pub fees: Fees,
    pub out_put: OutPutData,
    pub coin_vault: Pubkey, pub pc_vault: Pubkey,
    pub coin_mint: Pubkey, pub pc_mint: Pubkey,
    pub lp_mint: Pubkey, pub model_data_key: Pubkey,
    pub open_orders: Pubkey, pub serum_market: Pubkey,
    pub serum_program: Pubkey, pub target_orders: Pubkey,
    pub amm_admin: Pubkey,
    pub padding: [u64; 64],
}

/// Décode le compte principal d'un pool Stable Swap.
/// Retourne une structure PARTIELLE qui doit être complétée par le ModelDataAccount.
pub fn decode_pool_info(_address: &Pubkey, data: &[u8]) -> Result<(StableSwapAmmInfoData, f64)> {
    if data.len() != std::mem::size_of::<StableSwapAmmInfoData>() {
        bail!("Stable Swap Pool data length mismatch.");
    }
    let pool_struct: &StableSwapAmmInfoData = from_bytes(data);
    let total_fee_percent = if pool_struct.fees.trade_fee_denominator == 0 {
        0.0
    } else {
        pool_struct.fees.trade_fee_numerator as f64 / pool_struct.fees.trade_fee_denominator as f64
    };
    Ok((*pool_struct, total_fee_percent))
}


/// Décode le ModelDataAccount pour en extraire le facteur d'amplification.
pub fn decode_model_data(data: &[u8]) -> Result<u64> {
    if data.len() < 8 {
        bail!("ModelDataAccount data is too short.");
    }
    let amp_bytes: [u8; 8] = data[0..8].try_into()?;
    Ok(u64::from_le_bytes(amp_bytes))
}

#[async_trait]
impl PoolOperations for DecodedStableSwapPool {
    fn get_mints(&self) -> (Pubkey, Pubkey) {
        (self.mint_a, self.mint_b)
    }

    fn get_vaults(&self) -> (Pubkey, Pubkey) {
        (self.vault_a, self.vault_b)
    }
    fn address(&self) -> Pubkey { self.address }

    fn get_quote(&self, token_in_mint: &Pubkey, amount_in: u64, _current_timestamp: i64) -> Result<u64> {
        // Vérifier que le pool est hydraté (a des réserves et un amp)
        if self.amp == 0 || self.reserve_a == 0 || self.reserve_b == 0 {
            return Ok(0);
        }

        let (in_reserve, out_reserve) = if *token_in_mint == self.mint_a {
            (self.reserve_a, self.reserve_b)
        } else if *token_in_mint == self.mint_b {
            (self.reserve_b, self.reserve_a)
        } else {
            return Err(anyhow!("Input token does not belong to this pool."));
        };

        // Note: La logique de stableswap utilise des u128 pour la précision
        let amount_out = math::get_quote(
            amount_in as u128,
            in_reserve as u128,
            out_reserve as u128,
            self.amp
        )?;

        Ok(amount_out as u64)
    }
    async fn get_quote_async(&mut self, token_in_mint: &Pubkey, amount_in: u64, _rpc_client: &RpcClient) -> Result<u64> {
        self.get_quote(token_in_mint, amount_in, 0)
    }
}