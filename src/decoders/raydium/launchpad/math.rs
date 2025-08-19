// src/math/math

use uint::construct_uint;
use anyhow::{Result};

// Définit un type d'entier non signé de 256 bits
construct_uint! { pub struct U256(4); }

/// Calcule la racine carrée d'un U256 en utilisant la méthode de Newton.
/// C'est une méthode numérique rapide et précise pour les grands entiers.
fn sqrt(n: U256) -> U256 {
    if n.is_zero() { return U256::zero(); }
    // On commence avec une approximation (par exemple, la moitié du nombre de bits)
    let mut x = U256::one() << (n.bits() / 2);
    let mut y = (x + n / x) >> 1;
    while y < x {
        x = y;
        y = (x + n / x) >> 1;
    }
    x
}

/// Calcule le montant de sortie pour un swap sur une courbe linéaire P = a*x + b.
pub fn get_quote_linear_curve(
    amount_in: u64,
    total_base_sold: u64, // C'est le `x` initial
    slope_a: u64,
    initial_price_b: u64,
    is_buy: bool
) -> Result<u64> {
    let amount_in = U256::from(amount_in);
    let x_initial = U256::from(total_base_sold);
    let a = U256::from(slope_a);
    let b = U256::from(initial_price_b);
    let two = U256::from(2);

    if is_buy { // On achète des tokens de base (A) avec des tokens de quote (B)
        // On résout (a/2)*x_final^2 + b*x_final - [amount_in + V(x_initial)] = 0
        // où V(x) = (a*x^2)/2 + b*x
        let v_initial = (a * x_initial * x_initial) / two + (b * x_initial);
        let c_term = amount_in + v_initial;

        // Calcul du discriminant (delta) de l'équation du second degré: b^2 - 4*a*c
        // Ici, les coefficients sont A = a/2, B = b, C = -c_term
        let delta = (b * b) + (two * a * c_term);

        let sqrt_delta = sqrt(delta);

        if sqrt_delta <= b { return Ok(0); } // Pas de solution positive réelle

        let numerator = sqrt_delta - b;
        let denominator = a;
        if denominator.is_zero() { return Ok(0); } // Évite la division par zéro

        let x_final = numerator / denominator;

        if x_final <= x_initial { return Ok(0); }

        let amount_out = x_final - x_initial;
        Ok(amount_out.as_u64())

    } else { // On vend des tokens de base (A) pour des tokens de quote (B)
        // amount_out = V(x_final) - V(x_initial)
        let x_final = x_initial + amount_in;

        let v_initial = (a * x_initial * x_initial) / two + (b * x_initial);
        let v_final = (a * x_final * x_final) / two + (b * x_final);

        if v_final <= v_initial { return Ok(0); }
        Ok((v_final - v_initial).as_u64())
    }
}