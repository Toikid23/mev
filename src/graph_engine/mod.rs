use anyhow::Result;
use crate::rpc::ResilientRpcClient;
use solana_sdk::pubkey::Pubkey;
use std::collections::HashMap;

use crate::decoders::{Pool, PoolOperations};
use crate::decoders::{meteora, orca, pump, raydium};

// <-- MODIFIÉ : Nouvelle structure de Graphe, simple et clonable
#[derive(Default, Debug, Clone)]
pub struct Graph {
    // La map contient directement les pools. Plus de RwLock ou d'Arc ici.
    // L'immuabilité sera gérée par ArcSwap<Graph>.
    pub pools: HashMap<Pubkey, Pool>,
    pub account_to_pool_map: HashMap<Pubkey, Pubkey>,
}

impl Graph {
    pub fn new() -> Self {
        Self::default()
    }

    // <-- MODIFIÉ : La logique d'hydratation est maintenant une fonction associée (méthode de classe)
    // Elle ne modifie plus `self` mais retourne un nouveau pool hydraté.
    pub async fn hydrate_pool(pool: Pool, rpc_client: &ResilientRpcClient) -> Result<Pool> {
        match pool {
            Pool::RaydiumAmmV4(mut p) => {
                raydium::amm_v4::hydrate(&mut p, rpc_client).await?;
                Ok(Pool::RaydiumAmmV4(p))
            }
            Pool::RaydiumCpmm(mut p) => {
                raydium::cpmm::hydrate(&mut p, rpc_client).await?;
                Ok(Pool::RaydiumCpmm(p))
            }
            Pool::RaydiumClmm(mut p) => {
                raydium::clmm::hydrate(&mut p, rpc_client).await?;
                Ok(Pool::RaydiumClmm(p))
            }
            Pool::MeteoraDammV1(mut p) => {
                meteora::damm_v1::hydrate(&mut p, rpc_client).await?;
                Ok(Pool::MeteoraDammV1(p))
            }
            Pool::MeteoraDammV2(mut p) => {
                meteora::damm_v2::hydrate(&mut p, rpc_client).await?;
                Ok(Pool::MeteoraDammV2(p))
            }
            Pool::MeteoraDlmm(mut p) => {
                meteora::dlmm::hydrate(&mut p, rpc_client).await?;
                Ok(Pool::MeteoraDlmm(p))
            }
            Pool::OrcaWhirlpool(mut p) => {
                orca::whirlpool::hydrate(&mut p, rpc_client).await?;
                Ok(Pool::OrcaWhirlpool(p))
            }
            Pool::PumpAmm(mut p) => {
                pump::amm::hydrate(&mut p, rpc_client).await?;
                Ok(Pool::PumpAmm(p))
            }
        }
    }

    // <-- MODIFIÉ : Cette méthode prend maintenant `&mut self` et sera utilisée par le producteur sur sa copie clonée.
    pub fn add_pool_to_graph(&mut self, pool: Pool) {
        let (vault_a, vault_b) = pool.get_vaults();
        let pool_address = pool.address();
        self.account_to_pool_map.insert(vault_a, pool_address);
        self.account_to_pool_map.insert(vault_b, pool_address);
        self.account_to_pool_map.insert(pool_address, pool_address); // Important pour les CLMMs
        self.pools.insert(pool_address, pool);
    }

    // <-- MODIFIÉ : Méthode pour mettre à jour un pool existant dans le graphe.
    pub fn update_pool_in_graph(&mut self, pool_address: &Pubkey, new_pool_data: Pool) {
        if let Some(pool) = self.pools.get_mut(pool_address) {
            *pool = new_pool_data;
        }
    }
}