// src/decoders/meteora/damm_v1/math.rs

use anyhow::{anyhow, Result};
use uint::construct_uint;

// Pilier 1 : Précision Mathématique Absolue.
// On utilise un entier de 256 bits pour les calculs intermédiaires
// afin d'éviter tout débordement (overflow) et perte de précision.
construct_uint! { pub struct U256(4); }

/// Calcule l'invariant 'D' pour un pool stable à 2 tokens.
/// C'est une recherche de racine par la méthode de Newton, fidèle à l'implémentation de Curve.fi.
fn get_d(reserve_a: u128, reserve_b: u128, amp: u64) -> Result<u128> {
    let sum_x = reserve_a.checked_add(reserve_b).ok_or_else(|| anyhow!("Sum overflow in get_d"))?;
    if sum_x == 0 { return Ok(0); }

    let mut d = sum_x;
    let n_coins = U256::from(2);
    let ann = U256::from(amp) * n_coins;

    for _ in 0..64 { // 64 itérations sont plus que suffisantes pour la convergence.
        let d_u256 = U256::from(d);
        // Utilisation de U256 pour éviter les overflows sur les multiplications
        let ra_u256 = U256::from(reserve_a);
        let rb_u256 = U256::from(reserve_b);

        // d_p = d^(n+1) / (n^n * prod(x_i))
        // Pour n=2, prod(x_i) = reserve_a * reserve_b
        // d_p = d^3 / (4 * reserve_a * reserve_b)
        let d_p = (((d_u256 * d_u256) / (ra_u256 * n_coins)) * d_u256) / (rb_u256 * n_coins);
        let d_prev = d;

        // d = (Ann * S + n * d_p) / (Ann - 1 + (n+1) * d_p / d)
        let numerator = d_u256 * (ann * U256::from(sum_x) + d_p * n_coins);
        let denominator = (ann - U256::one()) * d_u256 + (n_coins + U256::one()) * d_p;

        d = (numerator / denominator).as_u128();

        // Si la différence est minime, on a convergé.
        if d.abs_diff(d_prev) <= 1 { break; }
    }
    Ok(d)
}

/// Calcule la nouvelle réserve de sortie 'y' en fonction de 'D' et de la nouvelle réserve d'entrée 'x'.
/// C'est une résolution d'équation du second degré, également par méthode de Newton.
fn get_y(x: u128, d: u128, amp: u64) -> Result<u128> {
    let n_coins = U256::from(2);
    let ann = U256::from(amp) * n_coins;
    let d_u256 = U256::from(d);
    let x_u256 = U256::from(x);

    // Résolution de : y^2 + y * (b - d) - c = 0
    // où :
    // b = S + D/Ann = x + D/Ann
    // c = D^(n+1) / (n^(2n) * P * Ann) = D^3 / (4 * x * Ann)
    let c = d_u256.pow(U256::from(3)) / (x_u256 * n_coins.pow(U256::from(2)) * ann);
    let b = x_u256 + d_u256 / ann;

    let mut y = d_u256;
    for _ in 0..64 {
        let y_prev = y;
        // Formule itérative de Newton : y_new = (y^2 + c) / (2y + b - d)
        let numerator = y.pow(U256::from(2)) + c;
        let denominator = y * U256::from(2) + b - d_u256;
        if denominator == U256::zero() { break; } // Évite la division par zéro
        y = numerator / denominator;
        if y == y_prev { break; }
    }
    Ok(y.as_u128())
}

/// Fonction principale qui encapsule la logique de calcul pour un Stable Swap.
pub fn get_quote(amount_in: u128, in_reserve: u128, out_reserve: u128, amp: u64) -> Result<u128> {
    if in_reserve == 0 || out_reserve == 0 { return Ok(0); }
    let d = get_d(in_reserve, out_reserve, amp)?;
    let new_in_reserve = in_reserve.checked_add(amount_in).ok_or_else(|| anyhow!("Amount in too large, overflow"))?;
    let new_out_reserve = get_y(new_in_reserve, d, amp)?;
    let amount_out = out_reserve.checked_sub(new_out_reserve).ok_or_else(|| anyhow!("Amount out underflow, likely slippage"))?;
    Ok(amount_out)
}