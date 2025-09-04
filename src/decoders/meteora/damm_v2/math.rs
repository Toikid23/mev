use anyhow::{anyhow, bail, Result};
use uint::construct_uint;
use super::pool::onchain_layouts; // Pour accéder aux structs de frais

construct_uint! { pub struct U256(4); }




// --- Fonctions de calcul des frais ---

pub fn get_base_fee(base_fee: &onchain_layouts::BaseFeeStruct, current_timestamp: i64, activation_point: u64) -> Result<u64> {
    if base_fee.period_frequency == 0 { return Ok(base_fee.cliff_fee_numerator); }
    let current_timestamp_u64 = u64::try_from(current_timestamp).unwrap_or(0);
    let period = if current_timestamp_u64 < activation_point {
        base_fee.number_of_period as u64
    } else {
        let elapsed = current_timestamp_u64.saturating_sub(activation_point);
        (elapsed / base_fee.period_frequency).min(base_fee.number_of_period as u64)
    };
    match base_fee.fee_scheduler_mode {
        0 => { let reduction = period.saturating_mul(base_fee.reduction_factor); Ok(base_fee.cliff_fee_numerator.saturating_sub(reduction)) }
        1 => { if base_fee.reduction_factor == 0 { return Ok(base_fee.cliff_fee_numerator); } let mut fee = base_fee.cliff_fee_numerator as u128; let reduction_factor = 10000 - base_fee.reduction_factor as u128; for _ in 0..period { fee = (fee * reduction_factor) / 10000; } Ok(fee as u64) }
        _ => bail!("Unsupported fee scheduler mode"),
    }
}

pub fn get_variable_fee(dynamic_fee: &onchain_layouts::DynamicFeeStruct) -> Result<u128> {
    let square_vfa_bin = dynamic_fee.volatility_accumulator.checked_mul(dynamic_fee.bin_step as u128).ok_or(anyhow!("MathOverflow"))?.checked_pow(2).ok_or(anyhow!("MathOverflow"))?;
    let v_fee = square_vfa_bin.checked_mul(dynamic_fee.variable_fee_control as u128).ok_or(anyhow!("MathOverflow"))?;
    let scaled_v_fee = v_fee.checked_add(99_999_999_999).ok_or(anyhow!("MathOverflow"))?.checked_div(100_000_000_000).ok_or(anyhow!("MathOverflow"))?;
    Ok(scaled_v_fee)
}

// --- Fonctions de calcul de swap (Quote et Required Input) ---

pub fn get_next_sqrt_price_from_input(sqrt_price: u128, liquidity: u128, amount_in: u64, a_for_b: bool) -> Result<u128> {
    if amount_in == 0 { return Ok(sqrt_price); }
    let sqrt_price_u256 = U256::from(sqrt_price);
    let liquidity_u256 = U256::from(liquidity);
    let amount_in_u256 = U256::from(amount_in);
    if a_for_b {
        let product = amount_in_u256.checked_mul(sqrt_price_u256).ok_or(anyhow!("MathOverflow"))?;
        let denominator = liquidity_u256.checked_add(product).ok_or(anyhow!("MathOverflow"))?;
        if denominator.is_zero() { return Err(anyhow!("Denominator is zero")); }
        let numerator = liquidity_u256.checked_mul(sqrt_price_u256).ok_or(anyhow!("MathOverflow"))?;
        let result = (numerator + denominator - U256::from(1)) / denominator;
        Ok(result.try_into().map_err(|_| anyhow!("TypeCastFailed"))?)
    } else {
        if liquidity_u256.is_zero() { return Err(anyhow!("Liquidity is zero")); }
        let quotient = (amount_in_u256 << 128) / liquidity_u256;
        let result = sqrt_price_u256.checked_add(quotient).ok_or(anyhow!("MathOverflow"))?;
        Ok(result.try_into().map_err(|_| anyhow!("TypeCastFailed"))?)
    }
}

pub fn get_amount_out(sqrt_price_start: u128, sqrt_price_end: u128, liquidity: u128, a_to_b: bool) -> Result<u64> {
    if a_to_b { // On fournit A, on reçoit B. Le prix baisse. start > end.
        get_delta_amount_b_unsigned(sqrt_price_end, sqrt_price_start, liquidity, Rounding::Down)
    } else { // On fournit B, on reçoit A. Le prix monte. start < end.
        get_delta_amount_a_unsigned(sqrt_price_start, sqrt_price_end, liquidity, Rounding::Down)
    }
}

pub fn get_next_sqrt_price_from_output(sqrt_price: u128, liquidity: u128, amount_out: u128, a_to_b: bool) -> Result<u128> {
    if liquidity == 0 { return Ok(sqrt_price); }
    let amount_out_u256 = U256::from(amount_out);
    let liquidity_u256 = U256::from(liquidity);

    if a_to_b {
        let delta_sqrt_price = (amount_out_u256 << 128) / liquidity_u256;
        Ok(sqrt_price.saturating_add(delta_sqrt_price.as_u128()))
    } else {
        let liquidity_shifted = liquidity_u256 << 64;
        let denominator = (liquidity_shifted / U256::from(sqrt_price)) + U256::from(amount_out);
        if denominator.is_zero() { return Err(anyhow!("Cannot calculate next sqrt_price from output")); }
        let next_sqrt_price = liquidity_shifted / denominator;
        Ok(next_sqrt_price.as_u128())
    }
}

pub fn get_amount_in(sqrt_price_start: u128, sqrt_price_end: u128, liquidity: u128, a_to_b: bool) -> Result<u64> {
    if a_to_b { // On veut B (output), on doit fournir A (input). Le prix baisse. start > end.
        get_delta_amount_a_unsigned(sqrt_price_end, sqrt_price_start, liquidity, Rounding::Up)
    } else { // On veut A (output), on doit fournir B (input). Le prix monte. start < end.
        get_delta_amount_b_unsigned(sqrt_price_start, sqrt_price_end, liquidity, Rounding::Up)
    }
}

// --- Helpers de bas niveau ---

#[derive(PartialEq, Clone, Copy)] pub enum Rounding { Up, Down }

fn get_delta_amount_a_unsigned(lower_sqrt_price: u128, upper_sqrt_price: u128, liquidity: u128, round: Rounding) -> Result<u64> {
    let result = get_delta_amount_a_unsigned_unchecked(lower_sqrt_price, upper_sqrt_price, liquidity, round)?;
    if result > U256::from(u64::MAX) { bail!("MathOverflow"); }
    Ok(result.try_into().map_err(|_| anyhow!("TypeCastFailed"))?)
}
fn get_delta_amount_a_unsigned_unchecked(lower_sqrt_price: u128, upper_sqrt_price: u128, liquidity: u128, round: Rounding) -> Result<U256> {
    const RESOLUTION: u8 = 128;
    let num_1 = U256::from(liquidity) << RESOLUTION;
    let den_1 = U256::from(lower_sqrt_price);
    let den_2 = U256::from(upper_sqrt_price);
    if den_1.is_zero() || den_2.is_zero() { bail!("Sqrt price is zero"); }
    let term_1 = num_1 / den_1;
    let term_2 = num_1 / den_2;
    let diff = term_1 - term_2;
    let result = match round {
        Rounding::Up => (diff + (U256::from(1) << RESOLUTION) - U256::from(1)) >> RESOLUTION,
        Rounding::Down => diff >> RESOLUTION,
    };
    Ok(result)
}
fn get_delta_amount_b_unsigned(lower_sqrt_price: u128, upper_sqrt_price: u128, liquidity: u128, round: Rounding) -> Result<u64> {
    let result = get_delta_amount_b_unsigned_unchecked(lower_sqrt_price, upper_sqrt_price, liquidity, round)?;
    if result > U256::from(u64::MAX) { bail!("MathOverflow"); }
    Ok(result.try_into().map_err(|_| anyhow!("TypeCastFailed"))?)
}
fn get_delta_amount_b_unsigned_unchecked(lower_sqrt_price: u128, upper_sqrt_price: u128, liquidity: u128, round: Rounding) -> Result<U256> {
    const RESOLUTION: u8 = 64;
    let liquidity_u256 = U256::from(liquidity);
    let delta_sqrt_price = U256::from(upper_sqrt_price - lower_sqrt_price);
    let prod = liquidity_u256.checked_mul(delta_sqrt_price).ok_or(anyhow!("MathOverflow"))?;
    match round {
        Rounding::Up => { let denominator = U256::from(1) << (RESOLUTION as usize) * 2; Ok((prod + denominator - U256::from(1)) / denominator) }
        Rounding::Down => Ok(prod >> (RESOLUTION as usize) * 2),
    }
}

