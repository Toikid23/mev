// DANS : src/execution/fee_manager.rs

use crate::rpc::ResilientRpcClient;
use solana_client::rpc_response::RpcPrioritizationFee;
use solana_sdk::pubkey::Pubkey;
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};
use tokio::sync::RwLock;
use tokio::task::JoinHandle;

const FEE_CACHE_REFRESH_INTERVAL_MS: u64 = 2000;

#[derive(Clone)]
pub struct FeeManager {
    // Le cache stocke les frais par compte. Clé: Pubkey du compte, Valeur: Fee le plus récent.
    fee_cache: Arc<RwLock<HashMap<Pubkey, RpcPrioritizationFee>>>,
    rpc_client: Arc<ResilientRpcClient>,
}

impl FeeManager {
    pub fn new(rpc_client: Arc<ResilientRpcClient>) -> Self {
        Self {
            fee_cache: Arc::new(RwLock::new(HashMap::new())),
            rpc_client,
        }
    }

    pub fn start(&self, hotlist_reader: Arc<RwLock<HashSet<Pubkey>>>) -> JoinHandle<()> {
        let fee_cache_clone = self.fee_cache.clone();
        let rpc_client_clone = self.rpc_client.clone();

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(FEE_CACHE_REFRESH_INTERVAL_MS));
            loop {
                interval.tick().await;

                let accounts_to_check = {
                    let reader = hotlist_reader.read().await;
                    reader.iter().cloned().collect::<Vec<_>>()
                };

                if accounts_to_check.is_empty() {
                    continue;
                }

                match rpc_client_clone.get_recent_prioritization_fees(&accounts_to_check).await {
                    Ok(fees) => {
                        let mut writer = fee_cache_clone.write().await;
                        // On ne vide pas complètement. On met à jour ou on insère.
                        // Cela garde en mémoire les frais pour des pools qui pourraient être
                        // temporairement inactifs dans la réponse RPC.
                        for (i, fee) in fees.into_iter().enumerate() {
                            if let Some(account_pubkey) = accounts_to_check.get(i) {
                                writer.insert(*account_pubkey, fee);
                            }
                        }
                    },
                    Err(e) => eprintln!("[FeeManager] Erreur de rafraîchissement des frais: {}", e),
                }
            }
        })
    }

    pub async fn calculate_priority_fee(&self, accounts_in_tx: &[Pubkey], overbid_percent: u8) -> u64 {
        let reader = self.fee_cache.read().await;
        if reader.is_empty() {
            return 1000;
        }

        let mut relevant_fees: Vec<u64> = accounts_in_tx
            .iter()
            .filter_map(|pubkey| reader.get(pubkey))
            .map(|fee| fee.prioritization_fee)
            .collect();

        if relevant_fees.is_empty() {
            relevant_fees = reader.values().map(|fee| fee.prioritization_fee).collect();
            if relevant_fees.is_empty() { return 1000; }
        }

        relevant_fees.sort_unstable();

        // --- LA STRATÉGIE FINALE : 80ème PERCENTILE + PRIME ---
        let percentile_index = (relevant_fees.len() as f64 * 0.8).floor() as usize;
        // S'assurer que l'index ne dépasse pas les bornes
        let safe_index = percentile_index.min(relevant_fees.len().saturating_sub(1));

        let base_fee = *relevant_fees.get(safe_index).unwrap_or(&1000);

        let overbid_amount = (base_fee as u128 * overbid_percent as u128) / 100;

        (base_fee as u128 + overbid_amount) as u64
    }
}