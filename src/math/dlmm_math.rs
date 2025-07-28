// DANS : src/math/dlmm_math.rs

use anyhow::Result;
use uint::construct_uint;

construct_uint! { pub struct U256(4); }


pub fn get_amount_out(
    amount_in: u64,
    price: u128,
    is_base_input: bool,
) -> Result<u64> {
    let amount_in_u256 = U256::from(amount_in);
    let price_u256 = U256::from(price);

    let amount_out_u256 = if is_base_input {
        (amount_in_u256 * price_u256) >> 64
    } else {
        if price_u256.is_zero() { return Ok(0); }
        (amount_in_u256 << 64) / price_u256
    };
    Ok(amount_out_u256.as_u64())
}

/// Calcule le montant de sortie pour un swap dans un seul bin DLMM.
pub fn get_amount_y(sqrt_price_a: u128, sqrt_price_b: u128, liquidity: u128) -> u128 {
    let (sqrt_price_a, sqrt_price_b) = if sqrt_price_a > sqrt_price_b { (sqrt_price_b, sqrt_price_a) } else { (sqrt_price_a, sqrt_price_b) };

    let delta_price = U256::from(sqrt_price_b - sqrt_price_a);
    let liquidity_u256 = U256::from(liquidity);

    // Formule: floor(liquidity * (sqrt_price_b - sqrt_price_a) / 2^64)
    ((liquidity_u256 * delta_price) >> 64).as_u128()
}

// REMPLACEZ get_amount_x par ceci
pub fn get_amount_x(sqrt_price_a: u128, sqrt_price_b: u128, liquidity: u128) -> u128 {
    let (sqrt_price_a, sqrt_price_b) = if sqrt_price_a > sqrt_price_b { (sqrt_price_b, sqrt_price_a) } else { (sqrt_price_a, sqrt_price_b) };

    if sqrt_price_a == 0 { return 0; } // Évite la division par zéro

    let liquidity_u256 = U256::from(liquidity);
    let numerator = liquidity_u256 << 64;
    let denominator = U256::from(sqrt_price_b);

    // Formule: floor(liquidity * 2^64 * (sqrt_price_b - sqrt_price_a) / (sqrt_price_b * sqrt_price_a))
    // C'est équivalent à: floor( ( (liquidity * 2^64 / sqrt_price_b) * (sqrt_price_b - sqrt_price_a) ) / sqrt_price_a )
    let ratio = numerator / denominator;
    let delta_price = U256::from(sqrt_price_b - sqrt_price_a);
    let result = (ratio * delta_price) / U256::from(sqrt_price_a);

    result.as_u128()
}


/// Calcule le taux de frais dynamique brut pour un bin spécifique.
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

    const VOLATILITY_FEE_PRECISION: u64 = 10_000;
    let base_fee_rate = (base_factor as u64).saturating_mul(bin_step as u64);

    let dynamic_fee_rate = (U256::from(capped_volatility)
        * U256::from(variable_fee_control)
        * U256::from(base_fee_rate)
        / U256::from(VOLATILITY_FEE_PRECISION)
    ).as_u64();

    Ok(dynamic_fee_rate)
}