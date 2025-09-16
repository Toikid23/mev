// DANS : src/decoders/pool_factory.rs

use crate::decoders::{meteora, orca, pump, raydium, Pool, PoolOperations};
use crate::graph_engine::Graph;
use crate::rpc::ResilientRpcClient;
use anyhow::{anyhow, Result};
use solana_sdk::{account::Account, pubkey::Pubkey};
use std::str::FromStr;
use std::sync::Arc;

/// La PoolFactory est responsable de la création et de l'hydratation des objets Pool.
/// Elle centralise la logique de mappage entre les ID de programme et les bons décodeurs.
#[derive(Clone)]
pub struct PoolFactory {
    rpc_client: Arc<ResilientRpcClient>,
}

impl PoolFactory {
    pub fn new(rpc_client: Arc<ResilientRpcClient>) -> Self {
        Self { rpc_client }
    }

    /// Crée et hydrate complètement un pool à partir de son adresse.
    /// C'est la méthode principale à utiliser lorsque vous découvrez un nouveau pool.
    pub async fn create_and_hydrate_pool(&self, pool_address: &Pubkey) -> Result<Pool> {
        let account = self.rpc_client.get_account(pool_address).await?;
        let raw_pool = self.decode_raw_pool(pool_address, &account.data, &account.owner)?;
        let hydrated_pool = Graph::hydrate_pool(raw_pool, &self.rpc_client).await?;
        Ok(hydrated_pool)
    }

    /// Tente de décoder les données brutes d'un compte en un objet `Pool` non hydraté.
    /// C'est utile pour les scanners/filtres qui n'ont pas besoin de données hydratées.
    pub fn decode_raw_pool(
        &self,
        address: &Pubkey,
        data: &[u8],
        owner: &Pubkey,
    ) -> Result<Pool> {
        match *owner {
            // Raydium
            id if id == raydium::amm_v4::RAYDIUM_AMM_V4_PROGRAM_ID => {
                raydium::amm_v4::decode_pool(address, data).map(Pool::RaydiumAmmV4)
            }
            id if id == Pubkey::from_str("CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C").unwrap() => {
                raydium::cpmm::decode_pool(address, data).map(Pool::RaydiumCpmm)
            }
            id if id == Pubkey::from_str("CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK").unwrap() => {
                raydium::clmm::decode_pool(address, data, &id).map(Pool::RaydiumClmm)
            }

            // Meteora
            id if id == Pubkey::from_str("Eo7WjKq67rjJQSZxS6z3YkapzY3eMj6Xy8X5EQVn5UaB").unwrap() => {
                meteora::damm_v1::decode_pool(address, data).map(Pool::MeteoraDammV1)
            }
            id if id == Pubkey::from_str("cpamdpZCGKUy5JxQXB4dcpGPiikHawvSWAd6mEn1sGG").unwrap() => {
                meteora::damm_v2::decode_pool(address, data).map(Pool::MeteoraDammV2)
            }
            id if id == meteora::dlmm::PROGRAM_ID => {
                meteora::dlmm::decode_lb_pair(address, data, &id).map(Pool::MeteoraDlmm)
            }

            // Orca
            id if id == Pubkey::from_str("whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc").unwrap() => {
                orca::whirlpool::decode_pool(address, data).map(Pool::OrcaWhirlpool)
            }

            // Pump
            id if id == pump::amm::PUMP_PROGRAM_ID => {
                pump::amm::decode_pool(address, data).map(Pool::PumpAmm)
            }

            _ => Err(anyhow!("Programme propriétaire inconnu: {}", owner)),
        }
    }
}