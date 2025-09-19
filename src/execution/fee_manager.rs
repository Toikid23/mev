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
use tracing::{error, info, warn, debug};

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
                    Err(e) => warn!(error = %e, "Erreur de rafraîchissement des frais de priorité."),
                }
            }
        })
    }

    pub async fn calculate_priority_fee(
        &self,
        accounts_in_tx: &[Pubkey],
        time_remaining_in_slot_ms: u128,
    ) -> u64 {
        let reader = self.fee_cache.read().await;
        if reader.is_empty() {
            // AJOUT : Log pour le cas du fallback
            debug!("Fee cache vide, utilisation du fallback de 1000 lamports.");
            return 1000; // Fallback s'il n'y a pas de données de frais
        }

        let mut relevant_fees: Vec<u64> = accounts_in_tx
            .iter()
            .filter_map(|pubkey| reader.get(pubkey))
            .map(|fee| fee.prioritization_fee)
            .collect();

        if relevant_fees.is_empty() {
            relevant_fees = reader.values().map(|fee| fee.prioritization_fee).collect();
            if relevant_fees.is_empty() {
                // AJOUT : Log pour le cas du fallback
                debug!("Aucun frais pertinent trouvé, utilisation du fallback de 1000 lamports.");
                return 1000;
            }
        }

        relevant_fees.sort_unstable();

        let percentile_index = (relevant_fees.len() as f64 * 0.8).floor() as usize;
        let safe_index = percentile_index.min(relevant_fees.len().saturating_sub(1));
        let base_fee = *relevant_fees.get(safe_index).unwrap_or(&1000);

        // --- NOUVELLE LOGIQUE DE COURBE D'ENCHÈRE ---
        let overbid_percent = match time_remaining_in_slot_ms {
            0..=100 => 200,   // < 100ms restant : Enchère très agressive (200% de plus)
            101..=250 => 50,  // < 250ms restant : Enchère agressive (50% de plus)
            251..=400 => 20,  // < 400ms restant : Enchère standard (20% de plus)
            _ => 10,          // > 400ms restant : Enchère conservatrice (10% de plus)
        };

        let overbid_amount = (base_fee as u128 * overbid_percent) / 100;
        let final_fee = (base_fee as u128 + overbid_amount) as u64;

        // --- LE BLOC DE LOGS CORRECT À AJOUTER ICI ---
        debug!(
            base_fee,
            time_remaining_ms = time_remaining_in_slot_ms,
            overbid_percent,
            final_fee,
            "Calcul des frais de priorité dynamique"
        );
        // --- FIN DU BLOC ---

        final_fee // On retourne la variable qu'on vient de créer
    }

    
    pub fn calculate_jito_tip(
        &self,
        estimated_profit: u64,
        tip_percent: u64,
        estimated_cus: u64,
    ) -> u64 {
        // Stratégie de base : un pourcentage du profit
        let profit_based_tip = (estimated_profit as u128 * tip_percent as u128 / 100) as u64;

        // Stratégie avancée : assurer un "tip par CU" minimum pour être compétitif.
        // Cette valeur (ex: 5000) peut être ajustée. Elle signifie qu'on offre
        // au moins 5000 lamports pour chaque 1000 CUs consommés.
        const MIN_TIP_PER_1000_CU: u64 = 5000;
        let cu_based_tip = (estimated_cus * MIN_TIP_PER_1000_CU) / 1000;

        // On prend le maximum des deux stratégies pour être sûr d'être compétitif,
        // tout en garantissant un tip minimum absolu de 10,000 lamports.
        profit_based_tip.max(cu_based_tip).max(10_000)
    }
}