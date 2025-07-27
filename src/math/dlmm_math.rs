// src/math/dlmm_math.rs

use anyhow::Result;
use uint::construct_uint;
construct_uint! { pub struct U256(4); }

/// Calcule le montant de sortie pour un swap à produit constant dans un bin.
pub fn get_amount_out(
    amount_in: u64,
    reserve_in: u128,
    reserve_out: u128,
) -> Result<u64> {
    if reserve_in == 0 {
        return Ok(0);
    }

    // Formule: amount_out = (amount_in * reserve_out) / (reserve_in + amount_in)
    let amount_in_u256 = U256::from(amount_in);
    let reserve_in_u256 = U256::from(reserve_in);
    let reserve_out_u256 = U256::from(reserve_out);

    let numerator = amount_in_u256 * reserve_out_u256;
    let denominator = reserve_in_u256 + amount_in_u256;

    if denominator.is_zero() { return Ok(0); }

    Ok((numerator / denominator).as_u64())
}