// src/decoders/mod.rs

use solana_sdk::pubkey::Pubkey;
use anyhow::Result;
use serde::{Serialize, Deserialize};

// --- 1. Déclarer tous nos modules principaux ---
pub mod pool_operations;
pub mod raydium;
pub mod orca;
pub mod meteora;
pub mod spl_token_decoders;
pub mod pump;

// --- 2. Importer le trait ---
pub use pool_operations::PoolOperations;

// --- 3. Définir l'enum unifié avec les BONS NOMS ---
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Pool {
    RaydiumAmmV4(raydium::amm_v4::DecodedAmmPool),
    RaydiumCpmm(raydium::cpmm::DecodedCpmmPool),
    RaydiumClmm(raydium::clmm::DecodedClmmPool),
    RaydiumStableSwap(raydium::stable_swap::DecodedStableSwapPool),
    RaydiumLaunchpad(raydium::launchpad::DecodedLaunchpadPool),
    MeteoraDammV1(meteora::damm_v1::DecodedMeteoraSbpPool),
    MeteoraDammV2(meteora::damm_v2::DecodedMeteoraDammPool),
    MeteoraDlmm(meteora::dlmm::DecodedDlmmPool),
    OrcaWhirlpool(orca::whirlpool::DecodedWhirlpoolPool),
    OrcaAmmV2(orca::amm_v2::DecodedOrcaAmmPool),
    OrcaAmmV1(orca::amm_v1::DecodedOrcaAmmV1Pool),
    PumpAmm(pump::amm::DecodedPumpAmmPool),
}

// --- 4. Implémenter le trait pour l'enum avec les BONS NOMS ---
impl PoolOperations for Pool {
    fn get_mints(&self) -> (Pubkey, Pubkey) {
        match self {
            Pool::RaydiumAmmV4(p) => p.get_mints(),
            Pool::RaydiumCpmm(p) => p.get_mints(),
            Pool::RaydiumClmm(p) => p.get_mints(),
            Pool::RaydiumStableSwap(p) => p.get_mints(),
            Pool::RaydiumLaunchpad(p) => p.get_mints(),
            Pool::MeteoraDammV1(p) => p.get_mints(), // <-- LA CORRECTION EST ICI
            Pool::MeteoraDammV2(p) => p.get_mints(),
            Pool::MeteoraDlmm(p) => p.get_mints(),
            Pool::OrcaWhirlpool(p) => p.get_mints(),
            Pool::OrcaAmmV2(p) => p.get_mints(),
            Pool::OrcaAmmV1(p) => p.get_mints(),
            Pool::PumpAmm(p) => p.get_mints(),
        }
    }

    fn get_vaults(&self) -> (Pubkey, Pubkey) {
        match self {
            Pool::RaydiumAmmV4(p) => p.get_vaults(),
            Pool::RaydiumCpmm(p) => p.get_vaults(),
            Pool::RaydiumClmm(p) => p.get_vaults(),
            Pool::RaydiumStableSwap(p) => p.get_vaults(),
            Pool::RaydiumLaunchpad(p) => p.get_vaults(),
            Pool::MeteoraDammV1(p) => p.get_vaults(), // <-- LA CORRECTION EST ICI
            Pool::MeteoraDammV2(p) => p.get_vaults(),
            Pool::MeteoraDlmm(p) => p.get_vaults(),
            Pool::OrcaWhirlpool(p) => p.get_vaults(),
            Pool::OrcaAmmV2(p) => p.get_vaults(),
            Pool::OrcaAmmV1(p) => p.get_vaults(),
            Pool::PumpAmm(p) => p.get_vaults(),
        }
    }

    fn get_quote(&self, token_in_mint: &Pubkey, amount_in: u64, current_timestamp: i64) -> Result<u64> {
        match self {
            Pool::RaydiumAmmV4(p) => p.get_quote(token_in_mint, amount_in, current_timestamp),
            Pool::RaydiumCpmm(p) => p.get_quote(token_in_mint, amount_in, current_timestamp),
            Pool::RaydiumClmm(p) => p.get_quote(token_in_mint, amount_in, current_timestamp),
            Pool::RaydiumStableSwap(p) => p.get_quote(token_in_mint, amount_in, current_timestamp),
            Pool::RaydiumLaunchpad(p) => p.get_quote(token_in_mint, amount_in, current_timestamp),
            Pool::MeteoraDammV1(p) => p.get_quote(token_in_mint, amount_in, current_timestamp), // <-- LA CORRECTION EST ICI
            Pool::MeteoraDammV2(p) => p.get_quote(token_in_mint, amount_in, current_timestamp),
            Pool::MeteoraDlmm(p) => p.get_quote(token_in_mint, amount_in, current_timestamp),
            Pool::OrcaWhirlpool(_) => {
                panic!("Ne pas utiliser get_quote synchrone pour OrcaWhirlpool.");
            },
            Pool::OrcaAmmV2(p) => p.get_quote(token_in_mint, amount_in, current_timestamp),
            Pool::OrcaAmmV1(p) => p.get_quote(token_in_mint, amount_in, current_timestamp),
            Pool::PumpAmm(p) => p.get_quote(token_in_mint, amount_in, current_timestamp),
        }
    }
}