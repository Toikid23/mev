use crate::decoders::{Pool, PoolOperations}; // On importe juste Pool et le trait
use crate::decoders::raydium_decoders::{amm_config, clmm_config, tick_array, launchpad, global_config, clmm_pool}; // Outils
use crate::decoders::meteora_decoders::dlmm;
use solana_client::rpc_client::RpcClient;
use solana_sdk::pubkey::Pubkey;
use std::collections::{HashMap, BTreeMap};
use anyhow::{Result, anyhow};

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
    pub fn hydrate_pool(&self, pool: Pool, rpc_client: &RpcClient) -> Result<Pool> {
        match pool {
            Pool::RaydiumAmmV4(mut p) => {
                println!("Hydrating Raydium AMM V4: {}", p.address);

                // 1. Lire les soldes des vaults pour les réserves
                let vaults_to_fetch = [p.vault_a, p.vault_b];
                let vault_accounts = rpc_client.get_multiple_accounts(&vaults_to_fetch)?;

                let vault_a_data = vault_accounts[0].as_ref().ok_or(anyhow!("Vault A not found"))?;
                let vault_b_data = vault_accounts[1].as_ref().ok_or(anyhow!("Vault B not found"))?;

                // Décoder le solde (offset 64 dans un compte token SPL)
                p.reserve_a = u64::from_le_bytes(vault_a_data.data[64..72].try_into()?);
                p.reserve_b = u64::from_le_bytes(vault_b_data.data[64..72].try_into()?);

                Ok(Pool::RaydiumAmmV4(p))
            },

            Pool::RaydiumCpmm(mut p) => {
                println!("Hydrating Raydium CPMM: {}", p.address);

                // 1. Lire le AmmConfig pour obtenir les frais
                let config_data = rpc_client.get_account_data(&p.amm_config)?;
                let decoded_config = amm_config::decode_config(&config_data)?;
                p.total_fee_percent = decoded_config.trade_fee_rate as f64 / 1_000_000.0;

                // 2. Lire les soldes des vaults pour les réserves
                let vaults_to_fetch = [p.token_0_vault, p.token_1_vault];
                let vault_accounts = rpc_client.get_multiple_accounts(&vaults_to_fetch)?;

                let vault_a_data = vault_accounts[0].as_ref().ok_or(anyhow!("CPMM Vault A not found"))?;
                let vault_b_data = vault_accounts[1].as_ref().ok_or(anyhow!("CPMM Vault B not found"))?;

                p.reserve_a = u64::from_le_bytes(vault_a_data.data[64..72].try_into()?);
                p.reserve_b = u64::from_le_bytes(vault_b_data.data[64..72].try_into()?);

                Ok(Pool::RaydiumCpmm(p))
            },

            Pool::RaydiumClmm(mut p) => {
                println!("Hydrating Raydium CLMM: {}", p.address);
                // Le chef d'orchestre délègue tout le travail à l'expert.
                clmm_pool::hydrate(&mut p, rpc_client)?;
                Ok(Pool::RaydiumClmm(p))
            },

            Pool::RaydiumLaunchpad(mut p) => {
                println!("Hydrating Raydium Launchpad: {}", p.address);

                let config_data = rpc_client.get_account_data(&p.global_config)?;
                let config = global_config::decode_global_config(&config_data)?;

                p.total_fee_percent = config.trade_fee_rate as f64 / 1_000_000.0;
                p.curve_type = match config.curve_type {
                    0 => launchpad::CurveType::ConstantProduct,
                    1 => launchpad::CurveType::FixedPrice,
                    2 => launchpad::CurveType::Linear,
                    _ => launchpad::CurveType::Unknown,
                };

                Ok(Pool::RaydiumLaunchpad(p))
            },

            Pool::MeteoraDlmm(mut p) => {
                println!("Hydrating Meteora DLMM: {}", p.address);

                // L'hydratation consiste à charger le BinArray actif pour obtenir les réserves.
                let bin_array_addr = dlmm::get_bin_array_address(&p.address, p.active_bin_id);

                // On utilise un `if let` car le BinArray peut ne pas exister (pas de liquidité)
                if let Ok(bin_array_data) = rpc_client.get_account_data(&bin_array_addr) {
                    if let Ok(bin) = dlmm::decode_bin_from_bin_array(p.active_bin_id, &bin_array_data) {
                        p.reserve_a = bin.amount_a;
                        p.reserve_b = bin.amount_b;
                    }
                }
                // Si le compte n'existe pas ou ne peut être décodé, les réserves restent à 0, ce qui est correct.

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
            _ => return,
        };

        self.account_to_pool_map.insert(vault_a, pool_address);
        self.account_to_pool_map.insert(vault_b, pool_address);
        self.pools.insert(pool_address, pool);

    }
}