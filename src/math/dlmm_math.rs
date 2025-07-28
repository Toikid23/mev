// src/math/dlmm_math.rs

use anyhow::Result;
use uint::construct_uint;

construct_uint! { pub struct U256(4); }

/// Calcule le montant de sortie pour un swap dans un seul bin DLMM, basé sur le prix du bin.
pub fn get_amount_out(
    amount_in: u64,
    price: u128,
    is_base_input: bool,
) -> Result<u64> {
    let amount_in_u256 = U256::from(amount_in);
    let price_u256 = U256::from(price);

    let amount_out_u256 = if is_base_input {
        // Vente de X pour Y: amount_out = amount_in * price
        let numerator = amount_in_u256.saturating_mul(price_u256);
        numerator >> 64
    } else {
        // Vente de Y pour X: amount_out = amount_in / price
        if price_u256.is_zero() { return Ok(0); }
        let numerator = amount_in_u256 << 64;
        numerator / price_u256
    };

    Ok(amount_out_u256.as_u64())
}

/// Calcule le taux de frais dynamique.
pub fn calculate_dynamic_fee(
    bins_crossed: u64,
    volatility_accumulator: u32,
    last_update_timestamp: i64,
    bin_step: u16,
    base_factor: u16,
    filter_period: u16,
    decay_period: u16,
    reduction_factor: u16,
    variable_fee_control: u32,
    max_volatility_accumulator: u32,
) -> Result<u64> {
    if bins_crossed == 0 { return Ok(0); }
    let current_timestamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs() as i64;
    let time_since_last_update = current_timestamp.saturating_sub(last_update_timestamp);
    let mut decayed_volatility = volatility_accumulator as u64;
    if time_since_last_update > 0 && time_since_last_update > filter_period as i64 {
        let elapsed_decay_periods = (time_since_last_update as u64) / (decay_period as u64);
        if elapsed_decay_periods > 0 {
            let reduction_basis_points = elapsed_decay_periods.saturating_mul(reduction_factor as u64);
            const BPS_SCALAR: u64 = 10000;
            if let Some(denominator) = BPS_SCALAR.checked_add(reduction_basis_points) {
                decayed_volatility = (U256::from(decayed_volatility) * U256::from(BPS_SCALAR) / U256::from(denominator)).as_u64();
            }
        }
    }
    let new_volatility = decayed_volatility.saturating_add(bins_crossed);
    let capped_volatility = new_volatility.min(max_volatility_accumulator as u64);
    const VOLATILITY_FEE_PRECISION: u128 = 10_000;
    let variable_fee_rate_u256 = U256::from(capped_volatility)
        .saturating_mul(U256::from(variable_fee_control))
        .saturating_mul(U256::from(base_factor))
        .saturating_mul(U256::from(bin_step))
        / (VOLATILITY_FEE_PRECISION);
    Ok(variable_fee_rate_u256.as_u64())
}