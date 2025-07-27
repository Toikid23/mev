// src/math/dlmm_math.rs

use uint::construct_uint;
use anyhow::{Result, anyhow};

construct_uint! { pub struct U256(4); }
const BPS_U128: u128 = 10_000;
const PRECISION_U128: u128 = 1 << 64; // Q64.64 fixed-point precision

fn mul_q64(a: u128, b: u128) -> u128 {
    ((U256::from(a) * U256::from(b)) >> 64).as_u128()
}

/// Calcule le prix d'un bin en utilisant des mathématiques sur u128 en virgule fixe Q64.64.
pub fn get_price_of_bin(bin_id: i32, bin_step: u16) -> Result<u128> { // <--- Accepte bin_step
    const ONE_Q64: u128 = 1 << 64;
    const BASIS_POINT_Q64: u128 = 18448588748116992571; // 1.0001

    // --- 1. Calculer la partie exponentielle (1.0001^bin_id) ---
    let power_of_bp = {
        if bin_id == 0 { ONE_Q64 }
        else {
            let mut base = BASIS_POINT_Q64;
            let mut result = ONE_Q64;
            let mut exponent = bin_id.abs();
            while exponent > 0 {
                if exponent % 2 == 1 { result = mul_q64(result, base); }
                base = mul_q64(base, base);
                exponent /= 2;
            }
            if bin_id < 0 {
                if result == 0 { return Err(anyhow!("Price calculation resulted in zero")); }
                ((U256::from(ONE_Q64) << 64) / U256::from(result)).as_u128()
            } else { result }
        }
    };

    // --- 2. Calculer la partie bin_step (1 + bin_step/10000) ---
    let bin_step_ratio = ONE_Q64 + ((U256::from(ONE_Q64) * U256::from(bin_step)) / U256::from(10000u16)).as_u128();

    // --- 3. Combiner les deux parties ---
    Ok(mul_q64(power_of_bp, bin_step_ratio))
}

/// Calcule le montant de sortie en utilisant la liquidité et le prix, avec des maths U256.
pub fn get_amount_out(
    amount_in: u64,
    active_bin_liquidity: u128,
    bin_price_q64: u128, // Le prix est au format Q64.64
    swap_for_y: bool,    // true si on vend X pour Y
) -> Result<u64> {
    // --- LA CORRECTION CLÉ EST ICI ---
    // On met amount_in à la même échelle que le prix (Q64.64)
    let amount_in_q64 = U256::from(amount_in) << 64;

    let active_bin_liquidity_u256 = U256::from(active_bin_liquidity);
    let bin_price_u256_q64 = U256::from(bin_price_q64);

    let result_q64 = if swap_for_y { // Vendre X pour Y
        if active_bin_liquidity == 0 { return Ok(0); }

        // y = L*P, x = L. dy = (y*dx) / (x+dx)
        // Tous les termes sont maintenant en Q64.64 ou U256
        let y = (active_bin_liquidity_u256 * bin_price_u256_q64) >> 64;
        let x = active_bin_liquidity_u256 << 64;
        let dx = amount_in_q64;

        let numerator = y * dx;
        let denominator = x + dx;
        if denominator.is_zero() { return Ok(0); }
        numerator / denominator

    } else { // Vendre Y pour X
        if active_bin_liquidity == 0 { return Ok(0); }

        // x = L, y = L*P. dx = (x*dy) / (y+dy)
        let x = active_bin_liquidity_u256 << 64;
        let y = (active_bin_liquidity_u256 * bin_price_u256_q64) >> 64;
        let dy = amount_in_q64;

        let numerator = x * dy;
        let denominator = y + dy;
        if denominator.is_zero() { return Ok(0); }
        numerator / denominator
    };

    // On reconvertit le résultat final de Q64.64 en un entier u64
    Ok((result_q64 >> 64).as_u64())
}