// DANS : src/decoders/meteora/damm_v1/math.rs

use anyhow::{anyhow, Result};
use ruint::aliases::U256;

fn get_d(reserve_a: u128, reserve_b: u128, amp: u64) -> Result<u128> {
    let sum_x = reserve_a.checked_add(reserve_b).ok_or_else(|| anyhow!("Sum overflow in get_d"))?;
    if sum_x == 0 { return Ok(0); }

    let mut d = sum_x;
    let n_coins: U256 = U256::from(2);
    let ann: U256 = U256::from(amp) * n_coins;

    for _ in 0..64 {
        let d_u256: U256 = U256::from(d);
        let ra_u256: U256 = U256::from(reserve_a);
        let rb_u256: U256 = U256::from(reserve_b);

        let d_p: U256 = (((d_u256 * d_u256) / (ra_u256 * n_coins)) * d_u256) / (rb_u256 * n_coins);
        let d_prev = d;

        let numerator: U256 = d_u256 * (ann * U256::from(sum_x) + d_p * n_coins);
        let denominator: U256 = (ann - U256::ONE) * d_u256 + (n_coins + U256::ONE) * d_p;

        d = (numerator / denominator).try_into()?;

        if d.abs_diff(d_prev) <= 1 { break; }
    }
    Ok(d)
}

fn get_y(x: u128, d: u128, amp: u64) -> Result<u128> {
    let n_coins: U256 = U256::from(2);
    let ann: U256 = U256::from(amp) * n_coins;
    let d_u256: U256 = U256::from(d);
    let x_u256: U256 = U256::from(x);

    let c: U256 = d_u256.pow(U256::from(3)) / (x_u256 * n_coins.pow(U256::from(2)) * ann);
    let b: U256 = x_u256 + d_u256 / ann;

    let mut y: U256 = d_u256;
    for _ in 0..64 {
        let y_prev = y;
        let numerator: U256 = y.pow(U256::from(2)) + c;
        let denominator: U256 = y * U256::from(2) + b - d_u256;
        if denominator.is_zero() { break; }
        y = numerator / denominator;
        if y == y_prev { break; }
    }
    Ok(y.try_into()?)
}

pub fn get_quote(amount_in: u128, in_reserve: u128, out_reserve: u128, amp: u64) -> Result<u128> {
    if in_reserve == 0 || out_reserve == 0 { return Ok(0); }
    let d = get_d(in_reserve, out_reserve, amp)?;
    let new_in_reserve = in_reserve.checked_add(amount_in).ok_or_else(|| anyhow!("Amount in too large, overflow"))?;
    let new_out_reserve = get_y(new_in_reserve, d, amp)?;
    let amount_out = out_reserve.checked_sub(new_out_reserve).ok_or_else(|| anyhow!("Amount out underflow, likely slippage"))?;
    Ok(amount_out)
}