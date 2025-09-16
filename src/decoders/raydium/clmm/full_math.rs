// DANS : src/decoders/raydium/clmm/full_math.rs

use ruint::aliases::{U128, U256, U512};

pub trait MulDiv<RHS = Self> {
    type Output;
    fn mul_div_floor(self, num: RHS, denom: RHS) -> Option<Self::Output>;
    fn mul_div_ceil(self, num: RHS, denom: RHS) -> Option<Self::Output>;
}

impl MulDiv for u64 {
    type Output = u64;
    fn mul_div_floor(self, num: Self, denom: Self) -> Option<Self::Output> {
        if denom == 0 { return None; }
        let r = (U128::from(self) * U128::from(num)) / U128::from(denom);
        r.try_into().ok()
    }
    fn mul_div_ceil(self, num: Self, denom: Self) -> Option<Self::Output> {
        if denom == 0 { return None; }
        let r = (U128::from(self) * U128::from(num) + (U128::from(denom) - U128::ONE)) / U128::from(denom);
        r.try_into().ok()
    }
}

impl MulDiv for U128 {
    type Output = U128;
    fn mul_div_floor(self, num: Self, denom: Self) -> Option<Self::Output> {
        if denom.is_zero() { return None; }
        let r = (U256::from(self) * U256::from(num)) / U256::from(denom);
        if r > U256::from(U128::MAX) { None } else {
            // On prend les 2 limbs de poids faible du U256 pour construire le U128
            let limbs = r.into_limbs();
            Some(U128::from_limbs([limbs[0], limbs[1]]))
        }
    }
    fn mul_div_ceil(self, num: Self, denom: Self) -> Option<Self::Output> {
        if denom.is_zero() { return None; }
        let r = (U256::from(self) * U256::from(num) + (U256::from(denom) - U256::ONE)) / U256::from(denom);
        if r > U256::from(U128::MAX) { None } else {
            let limbs = r.into_limbs();
            Some(U128::from_limbs([limbs[0], limbs[1]]))
        }
    }
}

impl MulDiv for U256 {
    type Output = U256;
    fn mul_div_floor(self, num: Self, denom: Self) -> Option<Self::Output> {
        if denom.is_zero() { return None; }
        let r = (U512::from(self) * U512::from(num)) / U512::from(denom);
        if r > U512::from(U256::MAX) { None } else {
            // On prend les 4 limbs de poids faible du U512 pour construire le U256
            let limbs = r.into_limbs();
            Some(U256::from_limbs([limbs[0], limbs[1], limbs[2], limbs[3]]))
        }
    }
    fn mul_div_ceil(self, num: Self, denom: Self) -> Option<Self::Output> {
        if denom.is_zero() { return None; }
        let r = (U512::from(self) * U512::from(num) + (U512::from(denom) - U512::ONE)) / U512::from(denom);
        if r > U512::from(U256::MAX) { None } else {
            let limbs = r.into_limbs();
            Some(U256::from_limbs([limbs[0], limbs[1], limbs[2], limbs[3]]))
        }
    }
}

pub trait DivCeil<RHS = Self> {
    fn div_ceil(self, other: RHS) -> Self;
}

impl DivCeil for U256 {
    fn div_ceil(self, other: Self) -> Self {
        (self + other - U256::ONE) / other
    }
}