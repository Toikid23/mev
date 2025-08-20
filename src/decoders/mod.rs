// src/decoders/mod.rs

use solana_sdk::pubkey::Pubkey;
use anyhow::Result;
use serde::{Serialize, Deserialize};
use solana_client::nonblocking::rpc_client::RpcClient; // <-- AJOUTER
use async_trait::async_trait;

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
#[async_trait]
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
            Pool::RaydiumStableSwap(p) => p.get_quote(token_in_mint, amount_in, current_timestamp),
            Pool::RaydiumLaunchpad(p) => p.get_quote(token_in_mint, amount_in, current_timestamp),
            Pool::MeteoraDammV1(p) => p.get_quote(token_in_mint, amount_in, current_timestamp),
            Pool::MeteoraDammV2(p) => p.get_quote(token_in_mint, amount_in, current_timestamp),
            Pool::OrcaAmmV2(p) => p.get_quote(token_in_mint, amount_in, current_timestamp),
            Pool::OrcaAmmV1(p) => p.get_quote(token_in_mint, amount_in, current_timestamp),
            Pool::PumpAmm(p) => p.get_quote(token_in_mint, amount_in, current_timestamp),

            // --- Pools Complexes (Synchrone NON FIABLE) ---
            Pool::RaydiumClmm(_) => Err(anyhow::anyhow!("Utiliser get_quote_async pour Raydium CLMM")),
            Pool::MeteoraDlmm(_) => Err(anyhow::anyhow!("Utiliser get_quote_async pour Meteora DLMM")),
            Pool::OrcaWhirlpool(_) => Err(anyhow::anyhow!("Utiliser get_quote_async pour Orca Whirlpool")),
        }
    }

    async fn get_quote_async(&mut self, token_in_mint: &Pubkey, amount_in: u64, rpc_client: &RpcClient) -> Result<u64> {
        match self {
            // On gère déjà le cas Whirlpool
            Pool::OrcaWhirlpool(p) => p.get_quote_with_rpc(token_in_mint, amount_in, rpc_client).await,

            // TODO: Implémenter la logique async pour CLMM et DLMM
            // Pour l'instant, ils vont utiliser leur `get_quote` synchrone qui est une estimation.
            // Le jour où on les intègrera, on créera leur propre `get_quote_with_rpc`.
            Pool::RaydiumClmm(p) => p.get_quote(token_in_mint, amount_in, 0),
            Pool::MeteoraDlmm(p) => p.get_quote(token_in_mint, amount_in, 0),

            // Pour les pools simples, on appelle simplement leur version synchrone
            _ => self.get_quote(token_in_mint, amount_in, 0),
        }
    }
}