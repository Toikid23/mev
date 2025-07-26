// src/math/dlmm_math.rs

use uint::construct_uint;
use anyhow::{Result, anyhow};

construct_uint! { pub struct U256(4); }
const BPS_U128: u128 = 10_000;
const PRECISION_U128: u128 = 1 << 64;

pub fn get_price_of_bin(bin_id: i32, bin_step: u16) -> Result<u128> {
    let bin_step_ratio = 1.0 + (bin_step as f64 / BPS_U128 as f64);
    let price_float = 1.0001f64.powi(bin_id) * bin_step_ratio;
    Ok((price_float * PRECISION_U128 as f64) as u128)
}

pub fn get_amount_out(
    amount_in: u64,
    active_bin_liquidity: u128,
    bin_price: u128,
    swap_for_y: bool,
) -> Result<u64> {
    let amount_in_u256 = U256::from(amount_in);
    let active_bin_liquidity_u256 = U256::from(active_bin_liquidity);
    let bin_price_u256 = U256::from(bin_price);
    let precision_u256 = U256::from(PRECISION_U128);

    if swap_for_y {
        if active_bin_liquidity == 0 { return Ok(0); }
        let price_x_liq = bin_price_u256 * active_bin_liquidity_u256;
        let denominator = (active_bin_liquidity_u256 * precision_u256) + (amount_in_u256 * bin_price_u256);
        if denominator.is_zero() { return Err(anyhow!("Division by zero")); }
        let amount_y = price_x_liq * amount_in_u256 / denominator;
        Ok(amount_y.as_u64())
    } else {
        let numerator = amount_in_u256 * active_bin_liquidity_u256 * precision_u256;
        let denominator = (active_bin_liquidity_u256 - amount_in_u256) * bin_price_u256;
        if denominator.is_zero() { return Err(anyhow!("Division by zero")); }
        let amount_x = numerator / denominator;
        Ok(amount_x.as_u64())
    }
}