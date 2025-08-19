// DANS : src/math/math

use anyhow::{anyhow, Result};
use uint::construct_uint;

// Pilier 1 : Précision Mathématique Absolue.
// On utilise un entier de 256 bits pour les calculs intermédiaires
// afin d'éviter tout débordement (overflow).
construct_uint! { pub struct U256(4); }

/// Calcule l'invariant 'D' pour un pool stable à 2 tokens.
/// C'est une recherche de racine par la méthode de Newton.
fn get_d(reserve_a: u128, reserve_b: u128, amp: u64) -> Result<u128> {
    let sum_x = reserve_a.checked_add(reserve_b).ok_or_else(|| anyhow!("Sum overflow in get_d"))?;
    if sum_x == 0 { return Ok(0); }

    let mut d = sum_x;
    let n_coins = U256::from(2);
    let ann = U256::from(amp) * n_coins;

    // 64 itérations suffisent pour une excellente convergence.
    for _ in 0..64 {
        let d_u256 = U256::from(d);
        let ra_u256 = U256::from(reserve_a);
        let rb_u256 = U256::from(reserve_b);

        // d_p = d^(n+1) / (n^n * product(x_i))
        let d_p = (((d_u256 * d_u256) / (ra_u256 * n_coins)) * d_u256) / (rb_u256 * n_coins);
        let d_prev = d;

        // d = (ann * sum_x + d_p * n) * d / ((ann - 1) * d + (n + 1) * d_p)
        let numerator = d_u256 * (ann * U256::from(sum_x) + d_p * n_coins);
        let denominator = (ann - U256::one()) * d_u256 + (n_coins + U256::one()) * d_p;

        d = (numerator / denominator).as_u128();
        if d == d_prev { break; } // Converged
    }
    Ok(d)
}

/// Calcule la nouvelle réserve de sortie 'y' en fonction de 'D' et de la nouvelle réserve d'entrée 'x'.
/// C'est également une recherche de racine par la méthode de Newton.
fn get_y(x: u128, d: u128, amp: u64) -> Result<u128> {
    let n_coins = U256::from(2);
    let ann = U256::from(amp) * n_coins;
    let d_u256 = U256::from(d);
    let x_u256 = U256::from(x);

    // c = D^(n+1) / (n^(2n) * product' * A)
    let c = d_u256.pow(U256::from(3)) / (x_u256 * n_coins.pow(U256::from(2)) * ann);
    // b = sum' + D / (A*n^n)
    let b = x_u256 + d_u256 / ann;

    let mut y = d_u256;
    for _ in 0..64 {
        let y_prev = y;
        // y = (y^2 + c) / (2y + b - D)
        let numerator = y.pow(U256::from(2)) + c;
        let denominator = y * U256::from(2) + b - d_u256;
        y = numerator / denominator;
        if y == y_prev { break; } // Converged
    }
    Ok(y.as_u128())
}

/// Fonction principale qui encapsule la logique de calcul pour un Stable Swap.
pub fn get_quote(amount_in: u64, in_reserve: u64, out_reserve: u64, amp: u64) -> Result<u64> {
    if in_reserve == 0 || out_reserve == 0 { return Ok(0); }

    let d = get_d(in_reserve as u128, out_reserve as u128, amp)?;
    let new_in_reserve = in_reserve.checked_add(amount_in).ok_or_else(|| anyhow!("Amount in too large"))?;
    let new_out_reserve = get_y(new_in_reserve as u128, d, amp)?;
    let amount_out = (out_reserve as u128).checked_sub(new_out_reserve).ok_or_else(|| anyhow!("Amount out underflow"))?;

    Ok(amount_out as u64)
}