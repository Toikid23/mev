// DANS: src/math/orca_whirlpool_math.rs

use uint::construct_uint;

// On définit un entier non signé de 256 bits pour les calculs intermédiaires.
construct_uint! { pub struct U256(4); }

// Constante pour la précision des sqrt_price (64 bits de partie fractionnaire).
const U128_MAX: u128 = u128::MAX;

/// Calcule la racine carrée du prix (sqrt_price) à partir d'un index de tick.
/// Cette implémentation est standard pour les CLMM.
pub fn tick_to_sqrt_price_x64(tick: i32) -> u128 {
    let abs_tick = tick.unsigned_abs();
    let mut ratio = if (abs_tick & 0x1) != 0 { 0xfffb023273ab_u128 } else { 0x1000000000000_u128 };
    if (abs_tick & 0x2) != 0 { ratio = (ratio * 0xfff608684f0a_u128) >> 48; }
    if (abs_tick & 0x4) != 0 { ratio = (ratio * 0xffeC11970624_u128) >> 48; }
    if (abs_tick & 0x8) != 0 { ratio = (ratio * 0xffd827226560_u128) >> 48; }
    if (abs_tick & 0x10) != 0 { ratio = (ratio * 0xffb0568d3568_u128) >> 48; }
    if (abs_tick & 0x20) != 0 { ratio = (ratio * 0xff610884848c_u128) >> 48; }
    if (abs_tick & 0x40) != 0 { ratio = (ratio * 0xfec21773228a_u128) >> 48; }
    // ... (le reste des multiplications peut être ajouté si des ticks plus grands sont nécessaires)

    if tick < 0 {
        ratio = U128_MAX / ratio;
    }
    ratio << 32
}

/// Calcule le montant de token Y (quote) nécessaire pour faire bouger le prix entre deux bornes.
/// Formule: Δy = L * (√Pb - √Pa)
pub fn get_delta_y(sqrt_price_a: u128, sqrt_price_b: u128, liquidity: u128) -> u128 {
    let (p_a, p_b) = if sqrt_price_a > sqrt_price_b { (sqrt_price_b, sqrt_price_a) } else { (sqrt_price_a, sqrt_price_b) };
    let delta_p = U256::from(p_b - p_a);
    let l = U256::from(liquidity);

    ((l * delta_p) >> 64).as_u128()
}

/// Calcule le montant de token X (base) nécessaire pour faire bouger le prix entre deux bornes.
/// Formule: Δx = L * (1/√Pa - 1/√Pb) = L * (√Pb - √Pa) / (√Pa * √Pb)
pub fn get_delta_x(sqrt_price_a: u128, sqrt_price_b: u128, liquidity: u128) -> u128 {
    let (p_a, p_b) = if sqrt_price_a > sqrt_price_b { (sqrt_price_b, sqrt_price_a) } else { (sqrt_price_a, sqrt_price_b) };
    let delta_p = U256::from(p_b - p_a);
    let l = U256::from(liquidity);

    let numerator = (l << 64) * delta_p;
    let denominator = U256::from(p_a) * U256::from(p_b);

    (numerator / denominator).as_u128()
}

/// Calcule le prochain sqrt_price en ajoutant du token X (le prix baisse).
pub fn get_next_sqrt_price_x_down(sqrt_price: u128, liquidity: u128, amount_in: u128) -> u128 {
    let l = U256::from(liquidity);
    let p = U256::from(sqrt_price);
    let a_in = U256::from(amount_in);

    // CORRECTION : On shift la liquidité de 64 bits AVANT de la multiplier par le prix.
    // Cela transforme le numérateur en un nombre au format X128.
    let numerator = (l << 64) * p; // Calcule X64 * X64 = X128
    let denominator = (l << 64) + a_in * p; // Calcule X64 + X64 = X64

    if denominator.is_zero() {
        return 0;
    }

    // Le résultat de X128 / X64 est bien un nombre au format X64, ce qui est correct.
    (numerator / denominator).as_u128()
}
/// Calcule le prochain sqrt_price en ajoutant du token Y (le prix monte).
pub fn get_next_sqrt_price_y_up(sqrt_price: u128, liquidity: u128, amount_in: u128) -> u128 {
    let l = U256::from(liquidity);
    let p = U256::from(sqrt_price);
    let a_in = U256::from(amount_in);

    (p + ((a_in << 64) / l)).as_u128()
}

/// Calcule l'étape d'un swap : montants et nouveau prix.
pub fn compute_swap_step(
    amount_remaining: u128,
    sqrt_price_current: u128,
    sqrt_price_target: u128,
    liquidity: u128,
    a_to_b: bool,
) -> (u128, u128, u128) {
    let amount_in: u128; // CORRECTION: `mut` retiré
    let amount_out: u128; // CORRECTION: `mut` retiré
    let next_sqrt_price: u128; // CORRECTION: `mut` retiré

    if a_to_b { // Swap de A vers B, le prix baisse
        let amount_needed_to_reach_target = get_delta_x(sqrt_price_target, sqrt_price_current, liquidity);
        amount_in = amount_remaining.min(amount_needed_to_reach_target);
        next_sqrt_price = get_next_sqrt_price_x_down(sqrt_price_current, liquidity, amount_in);
        amount_out = get_delta_y(next_sqrt_price, sqrt_price_current, liquidity);
    } else { // Swap de B vers A, le prix monte
        let amount_needed_to_reach_target = get_delta_y(sqrt_price_current, sqrt_price_target, liquidity);
        amount_in = amount_remaining.min(amount_needed_to_reach_target);
        next_sqrt_price = get_next_sqrt_price_y_up(sqrt_price_current, liquidity, amount_in);
        amount_out = get_delta_x(sqrt_price_current, next_sqrt_price, liquidity);
    }

    (amount_in, amount_out, next_sqrt_price)
}


const LOG_B_2_X32: i128 = 59543866431248i128;
const BIT_PRECISION: u32 = 14;
const LOG_B_P_ERR_MARGIN_LOWER_X64: i128 = 184467440737095516i128; // 0.01
const LOG_B_P_ERR_MARGIN_UPPER_X64: i128 = 15793534762490258745i128;

/// Dérive l'index du tick à partir d'un sqrt_price.
/// Copie exacte de la logique du SDK d'Orca.
pub fn sqrt_price_to_tick_index(sqrt_price_x64: u128) -> i32 {
    let msb: u32 = 128 - sqrt_price_x64.leading_zeros() - 1;
    let log2p_integer_x32 = (msb as i128 - 64) << 32;

    let mut bit: i128 = 0x8000_0000_0000_0000i128;
    let mut precision = 0;
    let mut log2p_fraction_x64 = 0;

    let mut r = if msb >= 64 {
        sqrt_price_x64 >> (msb - 63)
    } else {
        sqrt_price_x64 << (63 - msb)
    };

    while bit > 0 && precision < BIT_PRECISION {
        r *= r;
        let is_r_more_than_two = r >> 127_u32;
        r >>= 63 + is_r_more_than_two;
        log2p_fraction_x64 += bit * is_r_more_than_two as i128;
        bit >>= 1;
        precision += 1;
    }

    let log2p_fraction_x32 = log2p_fraction_x64 >> 32;
    let log2p_x32 = log2p_integer_x32 + log2p_fraction_x32;
    let logbp_x64 = log2p_x32 * LOG_B_2_X32;

    let tick_low: i32 = ((logbp_x64 - LOG_B_P_ERR_MARGIN_LOWER_X64) >> 64) as i32;
    let tick_high: i32 = ((logbp_x64 + LOG_B_P_ERR_MARGIN_UPPER_X64) >> 64) as i32;

    if tick_low == tick_high {
        tick_low
    } else {
        let actual_tick_high_sqrt_price_x64: u128 = tick_to_sqrt_price_x64(tick_high);
        if actual_tick_high_sqrt_price_x64 <= sqrt_price_x64 {
            tick_high
        } else {
            tick_low
        }
    }
}