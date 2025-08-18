// DANS: src/math/spl_token_swap_math.rs

use anyhow::{anyhow, Result};

/// Simulate a swap against a constant product curve, returning the amount
/// of destination tokens returned.
pub fn swap(
    source_amount: u128,
    swap_source_amount: u128,
    swap_destination_amount: u128,
) -> Result<u128> {
    let invariant = swap_source_amount.checked_mul(swap_destination_amount).ok_or_else(|| anyhow!("Invariant calculation failed"))?;
    let new_swap_source_amount = swap_source_amount.checked_add(source_amount).ok_or_else(|| anyhow!("New source amount calculation failed"))?;

    let (new_swap_destination_amount, _) = ceiling_div(invariant, new_swap_source_amount)?;

    let destination_amount_swapped = swap_destination_amount.checked_sub(new_swap_destination_amount).ok_or_else(|| anyhow!("Dest amount swapped calculation failed"))?;
    Ok(destination_amount_swapped)
}

// Helper for ceiling division
fn ceiling_div(a: u128, b: u128) -> Result<(u128, u128)> {
    if b == 0 {
        return Err(anyhow!("Division by zero"));
    }
    let mut quotient = a / b;
    let mut remainder = a % b;
    if remainder > 0 {
        quotient += 1;
        remainder = b - remainder;
    }
    Ok((quotient, remainder))
}