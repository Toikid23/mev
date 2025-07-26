// src/math/launchpad_math.rs

use uint::construct_uint;
use anyhow::{Result, anyhow};

construct_uint! { pub struct U256(4); }

/// Calcule la racine carrée d'un U256 en utilisant la méthode de Newton.
/// C'est une méthode numérique rapide et précise pour les grands entiers.
fn sqrt(n: U256) -> U256 {
    if n.is_zero() { return U256::zero(); }
    // On commence avec une approximation (n / 2)
    let mut x = n >> 1;
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
    total_base_sold: u64,
    slope: u64,
    initial_price: u64,
    is_buy: bool
) -> Result<u64> {

    let amount_in = U256::from(amount_in);
    let x_initial = U256::from(total_base_sold);
    let a = U256::from(slope);
    let b = U256::from(initial_price);
    let two = U256::from(2);

    if is_buy {
        let c_term_1 = amount_in;
        let c_term_2 = (a * x_initial * x_initial) / two;
        let c_term_3 = b * x_initial;
        let c_negative = c_term_1 + c_term_2 + c_term_3;
        let b_squared = b * b;
        let four_ac_negative = two * a * c_negative;
        let delta = b_squared + four_ac_negative;

        // On utilise NOTRE fonction sqrt
        let sqrt_delta = sqrt(delta);

        // Pour être sûr, on vérifie que b est plus petit que la racine
        if b >= sqrt_delta { return Ok(0); }

        let numerator = sqrt_delta - b;
        let x_final = numerator / a;

        if x_final <= x_initial { return Ok(0); }

        let amount_out = x_final - x_initial;
        Ok(amount_out.as_u64())

    } else {
        let x_final = x_initial + amount_in;
        let val_final = (a * x_final * x_final) / two + (b * x_final);
        let val_initial = (a * x_initial * x_initial) / two + (b * x_initial);

        Ok((val_final - val_initial).as_u64())
    }
}