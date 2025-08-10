use uint::{construct_uint}; // CORRECTION: On importe juste le constructeur
use anyhow::{Result, anyhow};
use super::full_math::MulDiv;

construct_uint! { pub struct U256(4); }
const BITS: u32 = 64;
const U128_MAX: u128 = 340282366920938463463374607431768211455;

mod tick_math {
    pub const MIN_TICK: i32 = -443636;
    pub const MAX_TICK: i32 = 443636;
}
pub use tick_math::{MIN_TICK, MAX_TICK};


// ... TOUT LE RESTE DU FICHIER EST IDENTIQUE ET CORRECT ...

// --- Fonctions de conversion Tick <-> SqrtPrice (Inchangées et correctes) ---
pub fn tick_to_sqrt_price_x64(tick: i32) -> u128 {
    // ... (votre fonction existante est correcte)
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

// --- Fonctions de calcul de montant (Inchangées et correctes) ---
fn get_amount_y(sqrt_price_a: u128, sqrt_price_b: u128, liquidity: u128) -> u128 {
    let (sqrt_price_a, sqrt_price_b) = if sqrt_price_a > sqrt_price_b { (sqrt_price_b, sqrt_price_a) } else { (sqrt_price_a, sqrt_price_b) };
    (U256::from(liquidity) * U256::from(sqrt_price_b - sqrt_price_a) >> BITS).as_u128()
}

fn get_amount_x(sqrt_price_a: u128, sqrt_price_b: u128, liquidity: u128) -> u128 {
    let (sqrt_price_a, sqrt_price_b) = if sqrt_price_a > sqrt_price_b { (sqrt_price_b, sqrt_price_a) } else { (sqrt_price_a, sqrt_price_b) };
    if sqrt_price_a == 0 { return 0; }
    let numerator = U256::from(liquidity) << (BITS * 2);
    let denominator = U256::from(sqrt_price_b) * U256::from(sqrt_price_a) >> BITS;
    if denominator.is_zero() { return 0; }
    let ratio = numerator / denominator;
    (ratio * U256::from(sqrt_price_b - sqrt_price_a) >> (BITS * 2)).as_u128()
}

fn get_next_sqrt_price_from_amount_x_in(sqrt_price: u128, liquidity: u128, amount_in: u128) -> u128 {
    if amount_in == 0 { return sqrt_price; }
    let numerator = U256::from(liquidity) << BITS;
    let product = U256::from(amount_in) * U256::from(sqrt_price);
    let denominator = numerator + product;
    if denominator.is_zero() { return sqrt_price; }
    (numerator * U256::from(sqrt_price) / denominator).as_u128()
}

fn get_next_sqrt_price_from_amount_y_in(sqrt_price: u128, liquidity: u128, amount_in: u128) -> u128 {
    if liquidity == 0 { return sqrt_price; }
    sqrt_price + ((U256::from(amount_in) << BITS) / U256::from(liquidity)).as_u128()
}

// --- NOUVELLE FONCTION `compute_swap_step` ---
// Calcule l'étape d'un swap en prenant en compte les frais.
// Retourne: (prochain_sqrt_price, montant_in_consommé, montant_out_produit, frais_payés)
pub fn compute_swap_step(
    sqrt_price_current_x64: u128,
    sqrt_price_target_x64: u128,
    liquidity: u128,
    amount_remaining: u128,
    fee_rate: u32,
    is_base_input: bool,
) -> Result<(u128, u128, u128, u128)> {

    let mut sqrt_price_next_x64: u128;
    let mut amount_in: u128 = 0;
    let mut amount_out: u128 = 0;
    let fee_rate_u64 = fee_rate as u64;
    const FEE_RATE_DENOMINATOR_VALUE: u64 = 1_000_000;

    let amount_remaining_less_fee = (amount_remaining as u64).mul_div_floor(
        FEE_RATE_DENOMINATOR_VALUE - fee_rate_u64,
        FEE_RATE_DENOMINATOR_VALUE
    ).ok_or_else(|| anyhow!("Math overflow"))? as u128;

    if is_base_input {
        let amount_in_to_reach_target = get_amount_x(sqrt_price_target_x64, sqrt_price_current_x64, liquidity);
        if amount_remaining_less_fee >= amount_in_to_reach_target {
            sqrt_price_next_x64 = sqrt_price_target_x64;
            amount_in = amount_in_to_reach_target;
        } else {
            sqrt_price_next_x64 = get_next_sqrt_price_from_amount_x_in(sqrt_price_current_x64, liquidity, amount_remaining_less_fee);
            amount_in = amount_remaining_less_fee;
        }
        amount_out = get_amount_y(sqrt_price_next_x64, sqrt_price_current_x64, liquidity);
    } else {
        let amount_in_to_reach_target = get_amount_y(sqrt_price_current_x64, sqrt_price_target_x64, liquidity);
        if amount_remaining_less_fee >= amount_in_to_reach_target {
            sqrt_price_next_x64 = sqrt_price_target_x64;
            amount_in = amount_in_to_reach_target;
        } else {
            sqrt_price_next_x64 = get_next_sqrt_price_from_amount_y_in(sqrt_price_current_x64, liquidity, amount_remaining_less_fee);
            amount_in = amount_remaining_less_fee;
        }
        amount_out = get_amount_x(sqrt_price_current_x64, sqrt_price_next_x64, liquidity);
    }

    let fee_amount = (amount_in as u64).mul_div_ceil(
        fee_rate_u64,
        FEE_RATE_DENOMINATOR_VALUE - fee_rate_u64
    ).ok_or_else(|| anyhow!("Math overflow"))? as u128;

    Ok((sqrt_price_next_x64, amount_in, amount_out, fee_amount))
}