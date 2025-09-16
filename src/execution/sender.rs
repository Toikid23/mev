// DANS : src/execution/sender.rs

use anyhow::{Result, Ok};
use async_trait::async_trait;
use solana_sdk::{signature::Signature, transaction::VersionedTransaction};
use crate::rpc::ResilientRpcClient;
use std::sync::Arc;
use tracing::info;

/// Représente les informations nécessaires pour envoyer une transaction d'arbitrage.
/// Cette structure possède ses données (pas d'emprunt), ce qui résout les problèmes de lifetime.
pub struct ArbitrageSendInfo {
    pub transaction: VersionedTransaction,
    pub is_jito_leader: bool,
    pub jito_tip: Option<u64>,
    pub estimated_profit: u64,
}

/// Le trait unifié pour l'envoi de transactions.
#[async_trait]
pub trait TransactionSender: Send + Sync {
    async fn send_transaction(&self, info: ArbitrageSendInfo) -> Result<Signature>;
}

// --- Notre SEUL service d'envoi "intelligent" ---

pub struct UnifiedSender {
    rpc_client: Arc<ResilientRpcClient>,
    dry_run: bool, // Si true, on n'envoie rien, on se contente d'afficher un log.
}

impl UnifiedSender {
    pub fn new(rpc_client: Arc<ResilientRpcClient>, dry_run: bool) -> Self {
        Self { rpc_client, dry_run }
    }
}

#[async_trait]
impl TransactionSender for UnifiedSender {
    async fn send_transaction(&self, info: ArbitrageSendInfo) -> Result<Signature> {
        if self.dry_run {
            let signature = info.transaction.signatures[0];
            if info.is_jito_leader {
                info!(
                    signature = %signature,
                    jito_tip = info.jito_tip.unwrap_or(0),
                    "[DRY RUN] Envoi d'un bundle Jito simulé."
                );
            } else {
                info!(
                    signature = %signature,
                    "[DRY RUN] Envoi d'une transaction RPC standard simulé."
                );
            }
            return Ok(signature);
        }

        // --- Logique d'envoi réelle ---
        if info.is_jito_leader {
            // Logique d'envoi Jito
            let jito_tip = info.jito_tip.unwrap_or(10_000);
            info!(
                profit_estimation = info.estimated_profit,
                jito_tip = jito_tip,
                "Envoi du bundle vers le Block Engine Jito..."
            );

            // TODO: Remplacer par la vraie logique d'envoi Jito.
            println!("[PLACEHOLDER] Le bundle Jito avec un tip de {} lamports serait envoyé ici.", jito_tip);

            // Pour l'instant, on retourne la signature de la tx principale pour simuler un succès.
            Ok(info.transaction.signatures[0])

        } else {
            // Logique d'envoi RPC standard
            info!(
                profit_estimation = info.estimated_profit,
                "Envoi de la transaction via RPC standard..."
            );
            self.rpc_client.send_and_confirm_transaction(&info.transaction).await
        }
    }
}