
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use solana_sdk::{signature::Signature, transaction::VersionedTransaction};
use crate::rpc::ResilientRpcClient;
use std::sync::Arc;
use tracing::info;
use serde_json::{json}; // Pour construire le corps de la requête JSON
use base64::{engine::general_purpose::STANDARD, Engine as _};




use crate::execution::routing::{JitoRegion, JITO_RPC_ENDPOINTS};



pub struct ArbitrageSendInfo {
    pub transaction: VersionedTransaction,
    pub jito_tip: Option<u64>,
    pub estimated_profit: u64,
    pub is_jito_tx: bool,
    pub target_jito_region: Option<JitoRegion>,
}

#[async_trait]
pub trait TransactionSender: Send + Sync {
    async fn send_transaction(&self, info: ArbitrageSendInfo) -> Result<Signature>;
}

// Le UnifiedSender est maintenant beaucoup plus simple
pub struct UnifiedSender {
    rpc_client: Arc<ResilientRpcClient>,
    // On a juste besoin d'un client HTTP pour parler à Jito
    http_client: reqwest::Client,
    dry_run: bool,
}

impl UnifiedSender {
    // Le constructeur n'a plus besoin de la keypair pour l'instant
    pub fn new(rpc_client: Arc<ResilientRpcClient>, dry_run: bool) -> Self {
        Self {
            rpc_client,
            http_client: reqwest::Client::new(),
            dry_run,
        }
    }

    // Fonction helper pour envoyer un bundle à une URL spécifique
    async fn send_jito_bundle_to_url(&self, transaction: &VersionedTransaction, url: &str) -> Result<()> {
        let serialized_tx = bincode::serialize(transaction)?;
        let encoded_tx = STANDARD.encode(serialized_tx);

        let json_payload = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "sendBundle",
            "params": [[encoded_tx]] // Jito attend un tableau de transactions encodées
        });

        let response = self.http_client.post(url)
            .json(&json_payload)
            .send()
            .await?;

        if !response.status().is_success() {
            let error_body = response.text().await?;
            return Err(anyhow!("Erreur Jito RPC ({}): {}", url, error_body));
        }

        // Pour l'envoi RPC, on ne s'attend pas à une réponse complexe, juste un succès
        Ok(())
    }
}



#[async_trait]
impl TransactionSender for UnifiedSender {
    async fn send_transaction(&self, info: ArbitrageSendInfo) -> Result<Signature> {
        let signature = info.transaction.signatures[0];

        if self.dry_run {
            if info.is_jito_tx {
                let region_log = info.target_jito_region.map_or("TOUTES les régions (fallback)".to_string(), |r| format!("la région ciblée : {:?}", r));
                info!(?signature, jito_tip = info.jito_tip.unwrap_or(0), "[DRY RUN] Envoi d'un bundle Jito simulé à {}", region_log);
            } else {
                info!(?signature, "[DRY RUN] Envoi d'une transaction RPC standard simulé.");
            }
            return Ok(signature);
        }

        if info.is_jito_tx {
            let bundle_tx = &info.transaction;

            // On envoie en parallèle à toutes les régions pertinentes
            // (Pour commencer, on envoie à toutes, le routage est une optimisation)
            let mut send_futures = Vec::new();
            for (region, url) in JITO_RPC_ENDPOINTS.iter() {
                // Si une région est ciblée, on envoie uniquement à celle-ci
                if info.target_jito_region.is_some() && info.target_jito_region.unwrap() != *region {
                    continue;
                }
                info!(?signature, ?region, "Envoi du bundle Jito...");
                send_futures.push(self.send_jito_bundle_to_url(bundle_tx, url));
            }

            // On attend que toutes les requêtes se terminent
            let results = futures_util::future::join_all(send_futures).await;

            // On vérifie si au moins un envoi a réussi
            if results.iter().all(|r| r.is_err()) {
                // Si tous ont échoué, on propage la première erreur
                return Err(anyhow!("Tous les envois de bundles Jito ont échoué. Première erreur: {:?}", results[0].as_ref().err()));
            }

        } else {
            // Transaction RPC normale
            info!(?signature, "Envoi de la transaction via RPC standard (mode rapide)...");
            return self.rpc_client.send_transaction_quick(&info.transaction).await;
        }

        Ok(signature)
    }
}