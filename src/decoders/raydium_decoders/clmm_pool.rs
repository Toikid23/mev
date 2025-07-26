use crate::decoders::pool_operations::PoolOperations;
use crate::decoders::raydium_decoders::tick_array::{self, TickArrayState, TickState};
use crate::math::clmm_math;
use bytemuck::{from_bytes, Pod, Zeroable};
use solana_sdk::pubkey::Pubkey;
use anyhow::{bail, Result, anyhow};
use std::collections::BTreeMap;
use serde::Deserialize;


// --- STRUCTURE PUBLIQUE FINALE ---
#[derive(Debug, Clone)]
pub struct DecodedClmmPool {
    pub address: Pubkey,
    pub program_id: Pubkey,
    pub amm_config: Pubkey,
    pub mint_a: Pubkey,
    pub mint_b: Pubkey,
    pub vault_a: Pubkey,
    pub vault_b: Pubkey,
    pub liquidity: u128,
    pub sqrt_price_x64: u128,
    pub tick_current: i32,
    pub tick_spacing: u16,
    pub total_fee_percent: f64,
    pub min_tick: i32,
    pub max_tick: i32,
    pub tick_arrays: Option<BTreeMap<i32, TickArrayState>>,
}


// --- STRUCTURES BRUTES (Miroir de l'IDL) ---

#[repr(C, packed)]
#[derive(Clone, Copy, Pod, Zeroable, Debug, Default)]
struct RewardInfo {
    pub reward_state: u8,
    pub open_time: u64,
    pub end_time: u64,
    pub last_update_time: u64,
    pub emissions_per_second_x64: u128,
    pub reward_total_emissioned: u64,
    pub reward_claimed: u64,
    pub token_mint: Pubkey,
    pub token_vault: Pubkey,
    pub authority: Pubkey,
    pub reward_growth_global_x64: u128,
}

#[repr(C, packed)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
struct PoolState {
    pub discriminator: [u64; 1],
    pub bump: [u8; 1],
    pub amm_config: Pubkey,
    pub owner: Pubkey,
    pub token_mint_0: Pubkey,
    pub token_mint_1: Pubkey,
    pub token_vault_0: Pubkey,
    pub token_vault_1: Pubkey,
    pub observation_key: Pubkey,
    pub mint_decimals_0: u8,
    pub mint_decimals_1: u8,
    pub tick_spacing: u16,
    pub liquidity: u128,
    pub sqrt_price_x64: u128,
    pub tick_current: i32,
    pub padding3: u16, // Champs de padding explicites de l'IDL
    pub padding4: u16,
    pub fee_growth_global_0_x64: u128,
    pub fee_growth_global_1_x64: u128,
    pub protocol_fees_token_0: u64,
    pub protocol_fees_token_1: u64,
    pub swap_in_amount_token_0: u128,
    pub swap_out_amount_token_1: u128,
    pub swap_in_amount_token_1: u128,
    pub swap_out_amount_token_0: u128,
    pub status: u8,
    pub padding: [u8; 7],
    pub reward_infos: [RewardInfo; 3],
    pub tick_array_bitmap: [u64; 16],
    pub total_fees_token_0: u64,
    pub total_fees_claimed_token_0: u64,
    pub total_fees_token_1: u64,
    pub total_fees_claimed_token_1: u64,
    pub fund_fees_token_0: u64,
    pub fund_fees_token_1: u64,
    pub open_time: u64,
    pub recent_epoch: u64,
    pub padding1: [u64; 24],
    pub padding2: [u64; 32],
}

/// Décode le compte PoolState d'un pool Raydium CLMM.
/// Décode un compte Raydium CLMM PoolState. Note: pas de discriminator.
pub fn decode_pool(address: &Pubkey, data: &[u8], program_id: &Pubkey) -> Result<DecodedClmmPool> {
    if data.len() != std::mem::size_of::<PoolState>() {
        bail!("PoolState data length mismatch. Expected {}, got {}",
            std::mem::size_of::<PoolState>(), data.len());
    }
    let pool_struct: &PoolState = from_bytes(data);

    Ok(DecodedClmmPool {
        address: *address,
        program_id: *program_id,
        amm_config: pool_struct.amm_config,
        mint_a: pool_struct.token_mint_0,
        mint_b: pool_struct.token_mint_1,
        vault_a: pool_struct.token_vault_0,
        vault_b: pool_struct.token_vault_1,
        liquidity: pool_struct.liquidity,
        sqrt_price_x64: pool_struct.sqrt_price_x64,
        tick_current: pool_struct.tick_current,
        tick_spacing: pool_struct.tick_spacing,
        total_fee_percent: 0.0, // Sera hydraté
        min_tick: -443636,
        max_tick: 443636,
        tick_arrays: None,
    })
}

impl PoolOperations for DecodedClmmPool {
    fn get_mints(&self) -> (Pubkey, Pubkey) { (self.mint_a, self.mint_b) }
    fn get_vaults(&self) -> (Pubkey, Pubkey) { (self.vault_a, self.vault_b) }
    fn get_quote(&self, _token_in_mint: &Pubkey, _amount_in: u64) -> Result<u64> {
        Err(anyhow!("Not implemented yet"))
    }
}

/// Calcule le montant de sortie pour un swap sur un pool CLMM.
pub fn get_clmm_quote(
    pool: &DecodedClmmPool,
    amount_in: u64,
    token_in_mint: &Pubkey,
    tick_arrays: &BTreeMap<i32, TickArrayState>
) -> Result<u64> {
    let mut amount_remaining = amount_in as u128;
    let mut total_amount_out: u128 = 0;

    let mut current_sqrt_price = pool.sqrt_price_x64;
    let mut current_tick_index = pool.tick_current;
    let mut current_liquidity = pool.liquidity;

    let is_base_input = *token_in_mint == pool.mint_a;

    while amount_remaining > 0 {
        let (next_tick_index, next_tick_liquidity_net) = find_next_initialized_tick(
            pool, current_tick_index, is_base_input, tick_arrays
        )?;

        let next_sqrt_price = clmm_math::tick_to_sqrt_price_x64(next_tick_index);

        let sqrt_price_target = if is_base_input {
            next_sqrt_price.max(clmm_math::tick_to_sqrt_price_x64(pool.min_tick))
        } else {
            next_sqrt_price.min(clmm_math::tick_to_sqrt_price_x64(pool.max_tick))
        };

        let amount_to_reach_target = if is_base_input {
            clmm_math::get_amount_x(sqrt_price_target, current_sqrt_price, current_liquidity)
        } else {
            clmm_math::get_amount_y(current_sqrt_price, sqrt_price_target, current_liquidity)
        };

        let amount_to_process = amount_remaining.min(amount_to_reach_target);

        if amount_to_process > 0 {
            let final_sqrt_price = if is_base_input {
                clmm_math::get_next_sqrt_price_from_amount_x_in(current_sqrt_price, current_liquidity, amount_to_process)
            } else {
                clmm_math::get_next_sqrt_price_from_amount_y_in(current_sqrt_price, current_liquidity, amount_to_process)
            };

            let amount_out_chunk = if is_base_input {
                clmm_math::get_amount_y(current_sqrt_price, final_sqrt_price, current_liquidity)
            } else {
                clmm_math::get_amount_x(final_sqrt_price, current_sqrt_price, current_liquidity)
            };

            total_amount_out += amount_out_chunk;
            amount_remaining -= amount_to_process;
            current_sqrt_price = final_sqrt_price;
        }

        if amount_remaining > 0 && current_sqrt_price == sqrt_price_target {
            current_liquidity = (current_liquidity as i128 + next_tick_liquidity_net) as u128;
            current_tick_index = next_tick_index;
        }
    }

    const PRECISION: u128 = 1_000_000;
    let fee_numerator = (pool.total_fee_percent * PRECISION as f64) as u128;
    let fee_to_deduct = (total_amount_out * fee_numerator) / PRECISION;

    Ok(total_amount_out.saturating_sub(fee_to_deduct) as u64)
}

// La fonction find_next_initialized_tick (corrigée)
fn find_next_initialized_tick(
    pool: &DecodedClmmPool,
    current_tick: i32,
    is_base_input: bool,
    tick_arrays: &BTreeMap<i32, TickArrayState>
) -> Result<(i32, i128)> {

    // On calcule l'index de l'array actuel une seule fois.
    let current_array_start_index = tick_array::get_start_tick_index(current_tick, pool.tick_spacing, 0);

    if is_base_input {
        // On cherche dans les arrays qui sont à l'index actuel ou en dessous
        for (_, array) in tick_arrays.range(..=current_array_start_index).rev() {
            for i in (0..tick_array::TICK_ARRAY_SIZE).rev() {
                let tick = &array.ticks[i];
                if tick.liquidity_gross != 0 && tick.tick < current_tick {
                    return Ok((tick.tick, tick.liquidity_net));
                }
            }
        }
    } else {
        // On cherche dans les arrays qui sont à l'index actuel ou au-dessus
        for (_, array) in tick_arrays.range(current_array_start_index..) {
            for i in 0..tick_array::TICK_ARRAY_SIZE {
                let tick = &array.ticks[i];
                if tick.liquidity_gross != 0 && tick.tick > current_tick {
                    return Ok((tick.tick, tick.liquidity_net));
                }
            }
        }
    }

    Err(anyhow!("No more initialized ticks in the swap direction within the provided arrays."))
}