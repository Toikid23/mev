// src/decoders/mod.rs

use solana_sdk::pubkey::Pubkey;
use anyhow::Result;
use crate::decoders::meteora_decoders::amm as meteora_amm;
// --- 1. Déclarer tous nos modules ---
pub mod pool_operations;
pub mod raydium_decoders;
pub mod orca_decoders;
pub mod meteora_decoders;
pub mod spl_token_decoders;
pub mod pump_decoders;


// --- 2. Importer le trait ---
pub use pool_operations::PoolOperations;

// --- 3. Définir l'enum unifié en utilisant les chemins complets ---
#[derive(Debug, Clone)]
pub enum Pool {
    RaydiumAmmV4(raydium_decoders::amm_v4::DecodedAmmPool),
    RaydiumCpmm(raydium_decoders::cpmm::DecodedCpmmPool),
    RaydiumClmm(raydium_decoders::clmm_pool::DecodedClmmPool),
    RaydiumStableSwap(raydium_decoders::stable_swap::DecodedStableSwapPool),
    RaydiumLaunchpad(raydium_decoders::launchpad::DecodedLaunchpadPool),
    MeteoraAmm(meteora_amm::DecodedMeteoraSbpPool),
    MeteoraDammV2(meteora_decoders::damm_v2::DecodedMeteoraDammPool),
    MeteoraDlmm(meteora_decoders::dlmm::DecodedDlmmPool),
    OrcaWhirlpool(orca_decoders::whirlpool_decoder::DecodedWhirlpoolPool),
    OrcaAmmV2(orca_decoders::token_swap_v2::DecodedOrcaAmmPool),
    OrcaAmmV1(orca_decoders::token_swap_v1::DecodedOrcaAmmV1Pool),
    PumpAmm(pump_decoders::amm::DecodedPumpAmmPool),

}

// --- 4. Implémenter le trait pour l'enum ---
impl PoolOperations for Pool {
    fn get_mints(&self) -> (Pubkey, Pubkey) {
        match self {
            Pool::RaydiumAmmV4(p) => p.get_mints(),
            Pool::RaydiumCpmm(p) => p.get_mints(),
            Pool::RaydiumClmm(p) => p.get_mints(),
            Pool::RaydiumStableSwap(p) => p.get_mints(),
            Pool::RaydiumLaunchpad(p) => p.get_mints(),
            Pool::MeteoraAmm(p) => p.get_mints(),
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
            Pool::MeteoraAmm(p) => p.get_vaults(),
            Pool::MeteoraDammV2(p) => p.get_vaults(),
            Pool::MeteoraDlmm(p) => p.get_vaults(),
            Pool::OrcaWhirlpool(p) => p.get_vaults(),
            Pool::OrcaAmmV2(p) => p.get_vaults(),
            Pool::OrcaAmmV1(p) => p.get_vaults(),
            Pool::PumpAmm(p) => p.get_vaults(),
        }
    }

    fn get_quote(&self, token_in_mint: &Pubkey, amount_in: u64, _current_timestamp: i64) -> Result<u64> {
        match self {
            Pool::RaydiumAmmV4(p) => p.get_quote(token_in_mint, amount_in, _current_timestamp),
            Pool::RaydiumCpmm(p) => p.get_quote(token_in_mint, amount_in, _current_timestamp),
            Pool::RaydiumClmm(p) => p.get_quote(token_in_mint, amount_in, _current_timestamp),
            Pool::RaydiumStableSwap(p) => p.get_quote(token_in_mint, amount_in, _current_timestamp),
            Pool::RaydiumLaunchpad(p) => p.get_quote(token_in_mint, amount_in, _current_timestamp),
            Pool::MeteoraAmm(p) => p.get_quote(token_in_mint, amount_in, _current_timestamp),
            Pool::MeteoraDammV2(p) => p.get_quote(token_in_mint, amount_in, _current_timestamp),
            Pool::MeteoraDlmm(p) => p.get_quote(token_in_mint, amount_in, _current_timestamp),
            Pool::OrcaWhirlpool(_) => {
                // On signale une erreur car cette voie ne doit pas être utilisée.
                panic!("Ne pas utiliser get_quote synchrone pour OrcaWhirlpool. Le chemin de test doit appeler la méthode async dédiée.");
            },
            Pool::OrcaAmmV2(p) => p.get_quote(token_in_mint, amount_in, _current_timestamp),
            Pool::OrcaAmmV1(p) => p.get_quote(token_in_mint, amount_in, _current_timestamp),
            Pool::PumpAmm(p) => p.get_quote(token_in_mint, amount_in, _current_timestamp),
        }
    }
}