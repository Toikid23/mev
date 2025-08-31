// Fichier : src/decoders/raydium/clmm/full_math.rs (VERSION FINALE CENTRALISÉE)

use uint::{construct_uint};
use num_integer::Integer; // Nécessaire pour u128::div_ceil

construct_uint! { pub struct U128(2); }
construct_uint! { pub struct U256(4); }
construct_uint! { pub struct U512(8); }

pub trait MulDiv<RHS = Self> {
    type Output;
    fn mul_div_floor(self, num: RHS, denom: RHS) -> Option<Self::Output>;
    fn mul_div_ceil(self, num: RHS, denom: RHS) -> Option<Self::Output>;
}

// Traits internes pour les conversions, pour garder le code propre
trait Upcast<T> { fn as_up(self) -> T; }
trait Downcast<T> { fn as_down(self) -> T; }

impl Upcast<U128> for u64 { fn as_up(self) -> U128 { U128::from(self) } }
impl Downcast<u64> for U128 { fn as_down(self) -> u64 { self.as_u64() } }
impl Upcast<U256> for U128 { fn as_up(self) -> U256 { U256([self.0[0], self.0[1], 0, 0]) } }
impl Downcast<U128> for U256 { fn as_down(self) -> U128 { U128([self.0[0], self.0[1]]) } }
impl Upcast<U512> for U256 { fn as_up(self) -> U512 { U512([self.0[0], self.0[1], self.0[2], self.0[3], 0, 0, 0, 0]) } }
impl Downcast<U256> for U512 { fn as_down(self) -> U256 { U256([self.0[0], self.0[1], self.0[2], self.0[3]]) } }

impl MulDiv for u64 {
    type Output = u64;
    fn mul_div_floor(self, num: Self, denom: Self) -> Option<Self::Output> {
        if denom == 0 { return None; }
        let r = (self.as_up() * num.as_up()) / denom.as_up();
        if r > u64::MAX.as_up() { None } else { Some(r.as_down()) }
    }
    fn mul_div_ceil(self, num: Self, denom: Self) -> Option<Self::Output> {
        if denom == 0 { return None; }
        let r = (self.as_up() * num.as_up() + (denom - 1).as_up()) / denom.as_up();
        if r > u64::MAX.as_up() { None } else { Some(r.as_down()) }
    }
}

impl MulDiv for U128 {
    type Output = U128;
    fn mul_div_floor(self, num: Self, denom: Self) -> Option<Self::Output> {
        if denom.is_zero() { return None; }
        let r = (self.as_up() * num.as_up()) / denom.as_up();
        if r > U128::MAX.as_up() { None } else { Some(r.as_down()) }
    }
    fn mul_div_ceil(self, num: Self, denom: Self) -> Option<Self::Output> {
        if denom.is_zero() { return None; }
        let r = (self.as_up() * num.as_up() + (denom - 1).as_up()) / denom.as_up();
        if r > U128::MAX.as_up() { None } else { Some(r.as_down()) }
    }
}

impl MulDiv for U256 {
    type Output = U256;
    fn mul_div_floor(self, num: Self, denom: Self) -> Option<Self::Output> {
        if denom.is_zero() { return None; }
        let r = (self.as_up() * num.as_up()) / denom.as_up();
        if r > U256::MAX.as_up() { None } else { Some(r.as_down()) }
    }
    fn mul_div_ceil(self, num: Self, denom: Self) -> Option<Self::Output> {
        if denom.is_zero() { return None; }
        let r = (self.as_up() * num.as_up() + (denom - 1).as_up()) / denom.as_up();
        if r > U256::MAX.as_up() { None } else { Some(r.as_down()) }
    }
}

// --- NOTRE PROPRE TRAIT POUR LA DIVISION PLAFOND ---
pub trait DivCeil<RHS = Self> {
    fn div_ceil(self, other: RHS) -> Self;
}

impl DivCeil for U256 {
    fn div_ceil(self, other: Self) -> Self {
        (self + other - U256::one()) / other
    }
}