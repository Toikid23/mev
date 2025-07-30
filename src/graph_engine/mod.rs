use crate::decoders::{Pool, PoolOperations}; // On importe juste Pool et le trait
use crate::decoders::raydium_decoders::{launchpad, clmm_pool, amm_v4, cpmm}; // Outils
use crate::decoders::meteora_decoders::dlmm;
use crate::decoders::orca_decoders::whirlpool_decoder;
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::pubkey::Pubkey;
use std::collections::{HashMap};
use anyhow::{Result};

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
    // Dans src/graph_engine/mod.rs

    // Rendez la fonction ASYNC et changez le type de rpc_client
    pub async fn hydrate_pool(&self, pool: Pool, rpc_client: &RpcClient) -> Result<Pool> {
        match pool {
            Pool::OrcaWhirlpool(mut p) => {
                println!("Hydrating Orca Whirlpool: {}", p.address);
                whirlpool_decoder::hydrate(&mut p, rpc_client).await?;
                Ok(Pool::OrcaWhirlpool(p))
            },


            Pool::RaydiumAmmV4(mut p) => {
                println!("Hydrating Raydium AMM V4: {}", p.address);
                // On délègue tout le travail à l'expert AMMv4
                amm_v4::hydrate(&mut p, rpc_client).await?;
                Ok(Pool::RaydiumAmmV4(p))
            },


            Pool::RaydiumCpmm(mut p) => {
                println!("Hydrating Raydium CPMM: {}", p.address);
                // On délègue tout le travail à l'expert CPMM
                cpmm::hydrate(&mut p, rpc_client).await?;
                Ok(Pool::RaydiumCpmm(p))
            },


            Pool::RaydiumClmm(mut p) => {
                println!("Hydrating Raydium CLMM: {}", p.address);
                // On délègue tout le travail à l'expert CLMM
                clmm_pool::hydrate(&mut p, rpc_client).await?;
                Ok(Pool::RaydiumClmm(p))
            },


            Pool::RaydiumLaunchpad(mut p) => {
                println!("Hydrating Raydium Launchpad: {}", p.address);
                // On délègue le travail à l'expert Launchpad
                launchpad::hydrate(&mut p, rpc_client).await?;
                Ok(Pool::RaydiumLaunchpad(p))
            },


            Pool::MeteoraDlmm(mut p) => {
                println!("Hydrating Meteora DLMM: {}", p.address);
                // On délègue le travail à l'expert DLMM
                dlmm::hydrate(&mut p, rpc_client, 10).await?;
                Ok(Pool::MeteoraDlmm(p))
            },


            _ => Ok(pool),
        }
    }

    // N'oubliez pas d'ajouter le variant au match dans `add_pool_to_graph`
    pub fn add_pool_to_graph(&mut self, pool: Pool) {
        let (vault_a, vault_b) = pool.get_vaults();
        let pool_address = match &pool {
            Pool::RaydiumAmmV4(p) => p.address,
            Pool::RaydiumCpmm(p) => p.address, // <--- AJOUTEZ CETTE LIGNE
            Pool::RaydiumClmm(p) => p.address,
            Pool::RaydiumLaunchpad(p) => p.address,
            Pool::MeteoraDlmm(p) => p.address,
            Pool::OrcaWhirlpool(p) => p.address,
            _ => return,
        };

        self.account_to_pool_map.insert(vault_a, pool_address);
        self.account_to_pool_map.insert(vault_b, pool_address);
        self.pools.insert(pool_address, pool);

    }
}