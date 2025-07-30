// DANS : src/math/clmm_math.rs

use uint::construct_uint;
use anyhow::{Result};

construct_uint! { pub struct U256(4); }
const BITS: u32 = 64;
const U128_MAX: u128 = 340282366920938463463374607431768211455;

// On définit nos constantes ici pour que tout soit au même endroit.
mod tick_math {
    pub const MIN_TICK: i32 = -443636;
    pub const MAX_TICK: i32 = 443636;
}

/// Calcule sqrt_price à partir d'un tick. (votre fonction, inchangée)
pub fn tick_to_sqrt_price_x64(tick: i32) -> u128 {
    let abs_tick = tick.unsigned_abs();
    let mut ratio = if (abs_tick & 0x1) != 0 { 0xfffb023273ab_u128 } else { 0x1000000000000_u128 };
    if (abs_tick & 0x2) != 0 { ratio = (ratio * 0xfff608684f0a_u128) >> 48; }
    if (abs_tick & 0x4) != 0 { ratio = (ratio * 0xffeC11970624_u128) >> 48; }
    if (abs_tick & 0x8) != 0 { ratio = (ratio * 0xffd827226560_u128) >> 48; }
    if (abs_tick & 0x10) != 0 { ratio = (ratio * 0xffb0568d3568_u128) >> 48; }
    if (abs_tick & 0x20) != 0 { ratio = (ratio * 0xff610884848c_u128) >> 48; }
    if (abs_tick & 0x40) != 0 { ratio = (ratio * 0xfec21773228a_u128) >> 48; }
    if (abs_tick & 0x80) != 0 { ratio = (ratio * 0xfe8459413444_u128) >> 48; }
    if (abs_tick & 0x100) != 0 { ratio = (ratio * 0xfd0901a55680_u128) >> 48; }
    if (abs_tick & 0x200) != 0 { ratio = (ratio * 0xfa13321be140_u128) >> 48; }
    if (abs_tick & 0x400) != 0 { ratio = (ratio * 0xf42f155989c0_u128) >> 48; }
    if (abs_tick & 0x800) != 0 { ratio = (ratio * 0xe8b33537b6c0_u128) >> 48; }
    if (abs_tick & 0x1000) != 0 { ratio = (ratio * 0xd1f3b2323f40_u128) >> 48; }
    if (abs_tick & 0x2000) != 0 { ratio = (ratio * 0xa53b6151c2c0_u128) >> 48; }
    if (abs_tick & 0x4000) != 0 { ratio = (ratio * 0x6b63304a7a80_u128) >> 48; }
    if (abs_tick & 0x8000) != 0 { ratio = (ratio * 0x247c3e533c80_u128) >> 48; }
    if (abs_tick & 0x10000) != 0 { ratio = (ratio * 0x028d7b32d180_u128) >> 48; }
    if (abs_tick & 0x20000) != 0 { ratio = (ratio * 0x000305565680_u128) >> 48; }
    if (abs_tick & 0x40000) != 0 { ratio = (ratio * 0x0000000936e0_u128) >> 48; }

    if tick < 0 {
        ratio = U128_MAX / ratio;
    }
    ratio << 32
}

// --- LA FONCTION DE CONVERSION SÉCURISÉE ET ROBUSTE ---
pub fn sqrt_price_x64_to_tick(sqrt_price_x64: u128) -> Result<i32> {
    // La formule est : prix = (sqrt_price / 2^64)^2
    // Et tick = log(base 1.0001, prix)

    // On convertit le prix en f64 pour le calcul du log, mais de manière SÉCURISÉE
    let price_f64 = (sqrt_price_x64 as f64) / ((1u64 << 32) as f64);
    let price_f64 = price_f64 * price_f64 / ( (1u64 << 32) as f64 * (1u64 << 32) as f64 );

    // GARDE-FOU n°1 : Gérer les cas où le prix devient invalide (trop grand ou trop petit)
    if !price_f64.is_finite() {
        return Ok(tick_math::MAX_TICK);
    }
    if price_f64 <= 0.0 {
        return Ok(tick_math::MIN_TICK);
    }

    // Calcul avec le logarithme
    let log_price = price_f64.log(1.0001);

    // On convertit en i128 pour avoir de la marge et éviter les overflows
    let final_tick = log_price.round() as i128;

    // GARDE-FOU n°2 : On s'assure que le résultat final est TOUJOURS dans les bornes valides
    Ok(final_tick.clamp(tick_math::MIN_TICK as i128, tick_math::MAX_TICK as i128) as i32)
}


// --- LE RESTE DES FONCTIONS MATHÉMATIQUES (inchangées) ---
pub fn get_amount_y(sqrt_price_a: u128, sqrt_price_b: u128, liquidity: u128) -> u128 {
    let (sqrt_price_a, sqrt_price_b) = if sqrt_price_a > sqrt_price_b { (sqrt_price_b, sqrt_price_a) } else { (sqrt_price_a, sqrt_price_b) };
    (U256::from(liquidity) * U256::from(sqrt_price_b - sqrt_price_a) >> BITS).as_u128()
}

pub fn get_amount_x(sqrt_price_a: u128, sqrt_price_b: u128, liquidity: u128) -> u128 {
    let (sqrt_price_a, sqrt_price_b) = if sqrt_price_a > sqrt_price_b { (sqrt_price_b, sqrt_price_a) } else { (sqrt_price_a, sqrt_price_b) };
    let numerator = U256::from(liquidity) << (BITS * 2);
    let denominator = U256::from(sqrt_price_b) * U256::from(sqrt_price_a) >> BITS;
    let ratio = numerator / denominator;
    (ratio * U256::from(sqrt_price_b - sqrt_price_a) >> (BITS * 2)).as_u128()
}

pub fn get_next_sqrt_price_from_amount_x_in(sqrt_price: u128, liquidity: u128, amount_in: u128) -> u128 {
    let numerator = U256::from(liquidity) << BITS;
    let product = U256::from(amount_in) * U256::from(sqrt_price);
    let denominator = numerator + product;
    (numerator * U256::from(sqrt_price) / denominator).as_u128()
}

pub fn get_next_sqrt_price_from_amount_y_in(sqrt_price: u128, liquidity: u128, amount_in: u128) -> u128 {
    sqrt_price + (amount_in << BITS) / liquidity
}

pub fn compute_swap_step(
    sqrt_price_current_x64: u128,
    sqrt_price_target_x64: u128,
    liquidity: u128,
    amount_remaining: u128,
    is_base_input: bool,
) -> (u128, u128, u128) {
    let mut amount_in = amount_remaining;
    let amount_out: u128;
    let next_sqrt_price_x64: u128;

    if is_base_input {
        let amount_to_reach_target = get_amount_x(sqrt_price_target_x64, sqrt_price_current_x64, liquidity);
        amount_in = amount_in.min(amount_to_reach_target);
        next_sqrt_price_x64 = get_next_sqrt_price_from_amount_x_in(sqrt_price_current_x64, liquidity, amount_in);
        amount_out = get_amount_y(next_sqrt_price_x64, sqrt_price_current_x64, liquidity);
    } else {
        let amount_to_reach_target = get_amount_y(sqrt_price_current_x64, sqrt_price_target_x64, liquidity);
        amount_in = amount_in.min(amount_to_reach_target);
        next_sqrt_price_x64 = get_next_sqrt_price_from_amount_y_in(sqrt_price_current_x64, liquidity, amount_in);
        amount_out = get_amount_x(sqrt_price_current_x64, next_sqrt_price_x64, liquidity);
    }

    (amount_in, amount_out, next_sqrt_price_x64)
}