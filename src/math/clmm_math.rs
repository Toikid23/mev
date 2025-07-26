// src/math/clmm_math.rs

use anyhow::Result;
use uint::construct_uint;

construct_uint! { pub struct U256(4); }
const BITS: u32 = 64;
const U128_MAX: u128 = 340282366920938463463374607431768211455;

/// Calcule sqrt_price à partir d'un tick. Traduction de la logique du SDK.
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
    (ratio << 32)
}

fn get_next_sqrt_price(price: u128, liquidity: u128, amount: u128, is_add: bool) -> u128 {
    if is_add {
        let amount_x = (amount << BITS) / liquidity;
        price + amount_x
    } else {
        let amount_x = (amount << BITS) / liquidity;
        price - amount_x
    }
}

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