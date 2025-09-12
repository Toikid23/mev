// src/graph_engine/mod.rs
// VERSION FINALE CORRIGÉE

use anyhow::Result;
use crate::rpc::ResilientRpcClient;
use solana_sdk::pubkey::Pubkey;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

// --- Imports principaux ---
// On importe le trait PoolOperations et l'enum unifié Pool
use crate::decoders::{Pool, PoolOperations};
// On importe les modules de premier niveau pour chaque protocole
use crate::decoders::{meteora, orca, pump, raydium};

#[derive(Default, Debug)]
pub struct Graph {
    // La map des pools est protégée par un RwLock pour permettre des lectures parallèles.
    // Chaque Pool est aussi dans un RwLock pour des mises à jour individuelles.
    pub pools: RwLock<HashMap<Pubkey, Arc<RwLock<Pool>>>>,
    pub account_to_pool_map: RwLock<HashMap<Pubkey, Pubkey>>,
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

    pub async fn add_pool_to_graph(&self, pool: Pool) {
        let (vault_a, vault_b) = pool.get_vaults();
        let pool_address = pool.address();
        let mut pools_writer = self.pools.write().await;
        let mut map_writer = self.account_to_pool_map.write().await;
        map_writer.insert(vault_a, pool_address);
        map_writer.insert(vault_b, pool_address);
        pools_writer.insert(pool_address, Arc::new(RwLock::new(pool)));
    }
}