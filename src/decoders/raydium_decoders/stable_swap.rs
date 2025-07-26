// src/decoders/raydium_decoders/stable_swap.rs

use crate::decoders::pool_operations::PoolOperations;
use bytemuck::{from_bytes, Pod, Zeroable};
use solana_sdk::pubkey::Pubkey;
use anyhow::{bail, Result, anyhow};

#[derive(Debug, Clone)]
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
}


// --- STRUCTURES BRUTES (Traduction de stable_stats.rs) ---

#[repr(C, packed)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
struct Fees {
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
struct OutPutData {
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
struct StableSwapAmmInfoData {
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
pub fn decode_pool_info(address: &Pubkey, data: &[u8]) -> Result<(StableSwapAmmInfoData, f64)> {
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

// --- IMPLEMENTATION DE LA LOGIQUE DU POOL ---
impl PoolOperations for DecodedStableSwapPool {
    fn get_mints(&self) -> (Pubkey, Pubkey) {
        (self.mint_a, self.mint_b)
    }

    fn get_vaults(&self) -> (Pubkey, Pubkey) {
        (self.vault_a, self.vault_b)
    }

    fn get_quote(&self, _token_in_mint: &Pubkey, _amount_in: u64) -> Result<u64> {
        // La logique de calcul pour un Stable Swap est très complexe et dépend de `amp`.
        // On la laisse en placeholder pour l'instant.
        Err(anyhow!("get_quote for Raydium Stable Swap is not implemented yet."))
    }
}