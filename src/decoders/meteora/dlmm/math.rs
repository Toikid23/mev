// DANS : src/decoders/meteora/dlmm/math.rs

use anyhow::Result;
use ruint::aliases::U256;

pub const FEE_PRECISION: u128 = 1_000_000_000;

pub fn get_amount_out(
    amount_in: u64,
    price: u128,
    is_base_input: bool,
) -> Result<u64> {
    let amount_in_u256: U256 = U256::from(amount_in);
    let price_u256: U256 = U256::from(price);

    let amount_out_u256: U256 = if is_base_input {
        (amount_in_u256 * price_u256) >> 64
    } else {
        if price_u256.is_zero() { return Ok(0); }
        (amount_in_u256 << 64) / price_u256
    };
    Ok(amount_out_u256.try_into().unwrap_or(0))
}

pub fn get_amount_in(
    amount_out: u64,
    price: u128,
    is_base_input: bool,
) -> Result<u64> {
    let amount_out_u256: U256 = U256::from(amount_out);
    let price_u256: U256 = U256::from(price);

    let amount_in_u256: U256 = if is_base_input {
        if price_u256.is_zero() { return Ok(u64::MAX); }
        (amount_out_u256 << 64) / price_u256
    } else {
        (amount_out_u256 * price_u256) >> 64
    };
    Ok(amount_in_u256.try_into().unwrap_or(u64::MAX))
}


pub fn get_amount_y(sqrt_price_a: u128, sqrt_price_b: u128, liquidity: u128) -> u128 {
    let (sqrt_price_a, sqrt_price_b) = if sqrt_price_a > sqrt_price_b { (sqrt_price_b, sqrt_price_a) } else { (sqrt_price_a, sqrt_price_b) };
    let delta_price: U256 = U256::from(sqrt_price_b - sqrt_price_a);
    let liquidity_u256: U256 = U256::from(liquidity);

    // LA CORRECTION : On utilise une variable intermédiaire pour spécifier le type.
    let result: U256 = (liquidity_u256 * delta_price) >> 64;
    result.try_into().unwrap_or(0)
}
pub fn get_amount_x(sqrt_price_a: u128, sqrt_price_b: u128, liquidity: u128) -> u128 {
    let (sqrt_price_a, sqrt_price_b) = if sqrt_price_a > sqrt_price_b { (sqrt_price_b, sqrt_price_a) } else { (sqrt_price_a, sqrt_price_b) };
    if sqrt_price_a == 0 { return 0; }
    let liquidity_u256: U256 = U256::from(liquidity);
    let numerator: U256 = liquidity_u256 << 64;
    let denominator: U256 = U256::from(sqrt_price_b);
    let ratio: U256 = numerator / denominator;
    let delta_price: U256 = U256::from(sqrt_price_b - sqrt_price_a);
    let result: U256 = (ratio * delta_price) / U256::from(sqrt_price_a);
    result.try_into().unwrap_or(0)
}


pub struct DynamicFeeParams {
    pub volatility_accumulator: u32,
    pub last_update_timestamp: i64,
    pub bin_step: u16,
    pub base_factor: u16,
    pub filter_period: u16,
    pub decay_period: u16,
    pub reduction_factor: u16,
    pub variable_fee_control: u32,
    pub max_volatility_accumulator: u32,
}


pub fn calculate_dynamic_fee(
    bins_crossed: u64,
    params: &DynamicFeeParams, // On passe la structure par référence
) -> Result<u64> {
    if bins_crossed == 0 { return Ok(0); }
    let current_timestamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs() as i64;
    // On accède aux champs via `params.`
    let time_since_last_update = current_timestamp.saturating_sub(params.last_update_timestamp);
    let mut decayed_volatility = params.volatility_accumulator as u64;

    if time_since_last_update > 0 && time_since_last_update > params.filter_period as i64 {
        let elapsed_decay_periods = (time_since_last_update as u64) / (params.decay_period as u64);
        if elapsed_decay_periods > 0 {
            let reduction_basis_points = elapsed_decay_periods.saturating_mul(params.reduction_factor as u64);
            const BPS_SCALAR: u64 = 10000;
            if let Some(denominator) = BPS_SCALAR.checked_add(reduction_basis_points) {
                let temp_volatility: ruint::aliases::U128 = ruint::aliases::U128::from(decayed_volatility) * ruint::aliases::U128::from(BPS_SCALAR) / ruint::aliases::U128::from(denominator);
                decayed_volatility = temp_volatility.try_into().unwrap_or(0);
            }
        }
    }

    let new_volatility = decayed_volatility.saturating_add(bins_crossed);
    let capped_volatility = new_volatility.min(params.max_volatility_accumulator as u64);
    const VOLATILITY_FEE_PRECISION: u64 = 10_000;
    let base_fee_rate = (params.base_factor as u64).saturating_mul(params.bin_step as u64);

    let dynamic_fee_rate_u256: U256 = U256::from(capped_volatility)
        * U256::from(params.variable_fee_control)
        * U256::from(base_fee_rate) / U256::from(VOLATILITY_FEE_PRECISION);

    let dynamic_fee_rate = dynamic_fee_rate_u256.try_into().unwrap_or(u64::MAX);
    Ok(dynamic_fee_rate)
}