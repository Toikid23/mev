// Fichier : src/math/full_math.rs (VERSION CORRIGÉE)

// CORRECTION : On utilise `uint::construct_uint` pour créer les types dont on a besoin localement.
// C'est la manière la plus robuste de faire.
use uint::{construct_uint};

construct_uint! {
    pub struct U128(2);
}
construct_uint! {
    pub struct U256(4);
}


/// Trait for calculating `val * num / denom` with different rounding modes and overflow
/// protection.
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
        if r > U128::from(u64::MAX) {
            None
        } else {
            Some(r.as_u64())
        }
    }

    fn mul_div_ceil(self, num: Self, denom: Self) -> Option<Self::Output> {
        if denom == 0 { return None; }
        let r = (U128::from(self) * U128::from(num) + U128::from(denom - 1)) / U128::from(denom);
        if r > U128::from(u64::MAX) {
            None
        } else {
            Some(r.as_u64())
        }
    }
}