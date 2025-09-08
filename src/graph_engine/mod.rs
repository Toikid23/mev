// src/graph_engine/mod.rs
// VERSION FINALE CORRIGÉE

use anyhow::Result;
use crate::rpc::ResilientRpcClient;
use solana_sdk::pubkey::Pubkey;
use std::collections::HashMap;

// --- Imports principaux ---
// On importe le trait PoolOperations et l'enum unifié Pool
use crate::decoders::{Pool, PoolOperations};
// On importe les modules de premier niveau pour chaque protocole
use crate::decoders::{meteora, orca, pump, raydium};

#[derive(Clone, Default)]
pub struct Graph {
    pub pools: HashMap<Pubkey, Pool>,
    pub account_to_pool_map: HashMap<Pubkey, Pubkey>,
}

impl Graph {
    pub fn new() -> Self {
        Self::default()
    }

    /// Prend un pool "brut", fait les appels RPC nécessaires,
    /// et retourne une version "hydratée" prête pour le graphe.
    pub async fn hydrate_pool(&self, pool: Pool, rpc_client: &ResilientRpcClient) -> Result<Pool> {
        match pool {
            // --- Raydium ---
            Pool::RaydiumAmmV4(mut p) => {
                println!("Hydrating Raydium AMM V4: {}", p.address);
                raydium::amm_v4::hydrate(&mut p, rpc_client).await?;
                Ok(Pool::RaydiumAmmV4(p))
            }
            Pool::RaydiumCpmm(mut p) => {
                println!("Hydrating Raydium CPMM: {}", p.address);
                raydium::cpmm::hydrate(&mut p, rpc_client).await?;
                Ok(Pool::RaydiumCpmm(p))
            }
            Pool::RaydiumClmm(mut p) => {
                println!("Hydrating Raydium CLMM: {}", p.address);
                raydium::clmm::hydrate(&mut p, rpc_client).await?;
                Ok(Pool::RaydiumClmm(p))
            }

            // --- Meteora ---
            // CORRECTION: On utilise le nouveau nom MeteoraDammV1 que vous aviez défini
            Pool::MeteoraDammV1(mut p) => {
                println!("Hydrating Meteora DAMM v1 (old AMM): {}", p.address);
                meteora::damm_v1::hydrate(&mut p, rpc_client).await?;
                Ok(Pool::MeteoraDammV1(p))
            }
            Pool::MeteoraDammV2(mut p) => {
                println!("Hydrating Meteora DAMM v2: {}", p.address);
                meteora::damm_v2::hydrate(&mut p, rpc_client).await?;
                Ok(Pool::MeteoraDammV2(p))
            }
            Pool::MeteoraDlmm(mut p) => {
                println!("Hydrating Meteora DLMM: {}", p.address);
                meteora::dlmm::hydrate(&mut p, rpc_client).await?;
                Ok(Pool::MeteoraDlmm(p))
            }

            // --- Orca ---
            Pool::OrcaWhirlpool(mut p) => {
                println!("Hydrating Orca Whirlpool: {}", p.address);
                orca::whirlpool::hydrate(&mut p, rpc_client).await?;
                Ok(Pool::OrcaWhirlpool(p))
            }

            // --- Pump.fun ---
            Pool::PumpAmm(mut p) => {
                println!("Hydrating pump.fun AMM: {}", p.address);
                pump::amm::hydrate(&mut p, rpc_client).await?;
                Ok(Pool::PumpAmm(p))
            }
        }
    }

    /// Ajoute un pool hydraté au graphe.
    pub fn add_pool_to_graph(&mut self, pool: Pool) {
        let (vault_a, vault_b) = pool.get_vaults();

        // Ce match a aussi été mis à jour pour utiliser les bons noms de variants
        let pool_address = match &pool {
            Pool::RaydiumAmmV4(p) => p.address,
            Pool::RaydiumCpmm(p) => p.address,
            Pool::RaydiumClmm(p) => p.address,
            Pool::MeteoraDammV1(p) => p.address,
            Pool::MeteoraDammV2(p) => p.address,
            Pool::MeteoraDlmm(p) => p.address,
            Pool::OrcaWhirlpool(p) => p.address,
            Pool::PumpAmm(p) => p.address,
        };

        self.account_to_pool_map.insert(vault_a, pool_address);
        self.account_to_pool_map.insert(vault_b, pool_address);
        self.pools.insert(pool_address, pool);
    }
}