// Fichier : src/decoders/raydium/clmm/math.rs (Version finale utilisant full_math)

use anyhow::{Result, anyhow};
use super::full_math::{self, MulDiv, U256, DivCeil}; // On importe notre nouveau trait

const BITS: u32 = 64;
const U128_MAX: u128 = 340282366920938463463374607431768211455;

pub mod tick_math {
    pub const MIN_TICK: i32 = -443636;
    pub const MAX_TICK: i32 = 443636;
    pub const MIN_SQRT_PRICE_X64: u128 = 4295048016;
    pub const MAX_SQRT_PRICE_X64: u128 = 79226673521066979257578248091;
}
pub use tick_math::{MIN_TICK, MAX_TICK};

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
        U128_MAX / ratio
    } else {
        ratio << 32
    }
}

fn div_rounding_up(x: U256, y: U256) -> U256 {
    (x + y - U256::one()) / y
}

pub fn get_amount_x(sqrt_price_a: u128, sqrt_price_b: u128, liquidity: u128, round_up: bool) -> Result<u64> {
    let (sqrt_price_a, sqrt_price_b) = if sqrt_price_a > sqrt_price_b { (sqrt_price_b, sqrt_price_a) } else { (sqrt_price_a, sqrt_price_b) };

    let numerator_1 = U256::from(liquidity) << BITS;
    let numerator_2 = U256::from(sqrt_price_b - sqrt_price_a);

    if sqrt_price_a == 0 { return Err(anyhow!("sqrt_price_a is zero")); }

    let result = if round_up {
        div_rounding_up(
            numerator_1.mul_div_ceil(numerator_2, U256::from(sqrt_price_b)).ok_or_else(|| anyhow!("muldiv error"))?,
            U256::from(sqrt_price_a)
        )
    } else {
        numerator_1.mul_div_floor(numerator_2, U256::from(sqrt_price_b)).ok_or_else(|| anyhow!("muldiv error"))?
            / U256::from(sqrt_price_a)
    };

    Ok(u64::try_from(result).map_err(|e| anyhow!(e))?)
}

pub fn get_amount_y(sqrt_price_a: u128, sqrt_price_b: u128, liquidity: u128, round_up: bool) -> Result<u64> {
    let (sqrt_price_a, sqrt_price_b) = if sqrt_price_a > sqrt_price_b { (sqrt_price_b, sqrt_price_a) } else { (sqrt_price_a, sqrt_price_b) };

    let result = if round_up {
        U256::from(liquidity).mul_div_ceil(
            U256::from(sqrt_price_b - sqrt_price_a),
            U256::from(1u128 << BITS),
        ).ok_or_else(|| anyhow!("muldiv error"))?
    } else {
        U256::from(liquidity).mul_div_floor(
            U256::from(sqrt_price_b - sqrt_price_a),
            U256::from(1u128 << BITS),
        ).ok_or_else(|| anyhow!("muldiv error"))?
    };

    Ok(u64::try_from(result).map_err(|e| anyhow!(e))?)
}

pub fn get_next_sqrt_price_from_amount_x_in(sqrt_price: u128, liquidity: u128, amount_in: u128, add: bool) -> u128 {
    if amount_in == 0 { return sqrt_price; }
    let numerator = U256::from(liquidity) << BITS;

    if add {
        let product = U256::from(amount_in) * U256::from(sqrt_price);
        if product / U256::from(amount_in) == U256::from(sqrt_price) {
            let denominator = numerator + product;
            if denominator >= numerator {
                return (numerator.mul_div_ceil(U256::from(sqrt_price), denominator).unwrap()).as_u128();
            }
        }
        (numerator / (numerator / U256::from(sqrt_price) + U256::from(amount_in))).as_u128()
    } else {
        let product = U256::from(amount_in) * U256::from(sqrt_price);
        let denominator = numerator.checked_sub(product).unwrap();
        (numerator.mul_div_ceil(U256::from(sqrt_price), denominator).unwrap()).as_u128()
    }
}

pub fn get_next_sqrt_price_from_amount_y_in(sqrt_price: u128, liquidity: u128, amount_in: u128, add: bool) -> u128 {
    if amount_in == 0 { return sqrt_price; }
    if add {
        let quotient = (U256::from(amount_in) << BITS) / U256::from(liquidity);
        (U256::from(sqrt_price) + quotient).as_u128()
    } else {
        let quotient = (U256::from(amount_in) << BITS).div_ceil(U256::from(liquidity));
        (U256::from(sqrt_price).checked_sub(quotient).unwrap()).as_u128()
    }
}

pub fn get_sqrt_price_from_amount_y_out(sqrt_price_current: u128, liquidity: u128, amount_out: u128) -> u128 {
    if liquidity == 0 { return sqrt_price_current; }
    let numerator = U256::from(amount_out) << BITS;
    let denominator = U256::from(liquidity);
    (U256::from(sqrt_price_current) - (numerator.div_ceil(denominator))).as_u128()
}

pub fn get_sqrt_price_from_amount_x_out(sqrt_price_current: u128, liquidity: u128, amount_out: u128) -> u128 {
    if liquidity == 0 || amount_out == 0 { return sqrt_price_current; }
    let numerator = U256::from(liquidity) << BITS;
    let denominator = (numerator / U256::from(sqrt_price_current)).checked_add(U256::from(amount_out)).unwrap();
    (numerator.div_ceil(denominator)).as_u128()
}

pub fn compute_swap_step(
    sqrt_price_current_x64: u128,
    sqrt_price_target_x64: u128,
    liquidity: u128,
    amount_remaining: u128,
    fee_rate: u32,
    is_base_input: bool,
) -> Result<(u128, u128, u128, u128)> {
    let mut sqrt_price_next_x64: u128;
    let mut amount_in: u128;
    let mut amount_out: u128;
    let fee_rate_u64 = fee_rate as u64;
    const FEE_RATE_DENOMINATOR_VALUE: u64 = 1_000_000;

    let amount_remaining_less_fee = (amount_remaining as u64).mul_div_floor(
        FEE_RATE_DENOMINATOR_VALUE - fee_rate_u64,
        FEE_RATE_DENOMINATOR_VALUE
    ).ok_or_else(|| anyhow!("Math overflow"))? as u128;

    if is_base_input {
        // On fournit du token de base (X), on veut du token de quote (Y)
        let amount_in_to_reach_target = get_amount_x(sqrt_price_target_x64, sqrt_price_current_x64, liquidity, true)? as u128;
        if amount_remaining_less_fee >= amount_in_to_reach_target {
            sqrt_price_next_x64 = sqrt_price_target_x64;
            amount_in = amount_in_to_reach_target;
        } else {
            sqrt_price_next_x64 = get_next_sqrt_price_from_amount_x_in(sqrt_price_current_x64, liquidity, amount_remaining_less_fee, true);
            amount_in = amount_remaining_less_fee;
        }
        // Pour un output, on arrondit toujours AU PLANCHER (round_up = false)
        amount_out = get_amount_y(sqrt_price_next_x64, sqrt_price_current_x64, liquidity, false)? as u128;
    } else {
        // On fournit du token de quote (Y), on veut du token de base (X)
        let amount_in_to_reach_target = get_amount_y(sqrt_price_current_x64, sqrt_price_target_x64, liquidity, true)? as u128;
        if amount_remaining_less_fee >= amount_in_to_reach_target {
            sqrt_price_next_x64 = sqrt_price_target_x64;
            amount_in = amount_in_to_reach_target;
        } else {
            sqrt_price_next_x64 = get_next_sqrt_price_from_amount_y_in(sqrt_price_current_x64, liquidity, amount_remaining_less_fee, true);
            amount_in = amount_remaining_less_fee;
        }
        // Pour un output, on arrondit toujours AU PLANCHER (round_up = false)
        amount_out = get_amount_x(sqrt_price_current_x64, sqrt_price_next_x64, liquidity, false)? as u128;
    }

    // Les frais sont calculés sur l'input NET, avec arrondi au PLAFOND
    let fee_amount = (amount_in as u64).mul_div_ceil(
        fee_rate_u64,
        FEE_RATE_DENOMINATOR_VALUE - fee_rate_u64
    ).ok_or_else(|| anyhow!("Math overflow"))? as u128;

    Ok((sqrt_price_next_x64, amount_in, amount_out, fee_amount))
}

pub fn get_tick_at_sqrt_price(sqrt_price_x64: u128) -> Result<i32> {
    if !(sqrt_price_x64 >= tick_math::MIN_SQRT_PRICE_X64 && sqrt_price_x64 < tick_math::MAX_SQRT_PRICE_X64) {
        return Err(anyhow!("SqrtPrice out of range"));
    }

    let msb = 128 - sqrt_price_x64.leading_zeros() - 1;
    let log2p_integer_x32 = (msb as i128 - 64) << 32;

    let mut bit: i128 = 0x8000_0000_0000_0000i128;
    let mut precision = 0;
    let mut log2p_fraction_x64 = 0;

    let mut r = if msb >= 64 {
        sqrt_price_x64 >> (msb - 63)
    } else {
        sqrt_price_x64 << (63 - msb)
    };

    while bit > 0 && precision < 16 { // BIT_PRECISION = 16
        r *= r;
        let is_r_more_than_two = r >> 127_u32;
        r >>= 63 + is_r_more_than_two;
        log2p_fraction_x64 += bit * is_r_more_than_two as i128;
        bit >>= 1;
        precision += 1;
    }

    let log2p_fraction_x32 = log2p_fraction_x64 >> 32;
    let log2p_x32 = log2p_integer_x32 + log2p_fraction_x32;

    let log_sqrt_10001_x64 = log2p_x32 * 59543866431248i128; // LOG_B_2_X32

    // tick - 0.01
    let tick_low: i32 = ((log_sqrt_10001_x64 - 184467440737095516i128) >> 64) as i32;

    // tick + (2^-14 / log2(√1.0001)) + 0.01
    let tick_high: i32 = ((log_sqrt_10001_x64 + 15793534762490258745i128) >> 64) as i32;

    Ok(if tick_low == tick_high {
        tick_low
    } else if tick_to_sqrt_price_x64(tick_high) <= sqrt_price_x64 {
        tick_high
    } else {
        tick_low
    })
}