// DANS : src/execution/confirmation_service.rs

use crate::rpc::ResilientRpcClient;
use crate::monitoring::metrics;
use crate::execution::simulator::parse_realized_profit_from_logs;
use anyhow::{Context, Result};
use solana_sdk::signature::Signature;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::{self, Receiver, Sender};
use tokio::task::JoinHandle;
use tracing::{error, info, warn};

// Timeout après lequel on considère une transaction comme perdue si elle n'est pas confirmée.
const TRANSACTION_TIMEOUT_SECS: u64 = 60;

/// Contient les informations nécessaires pour suivre une transaction envoyée
/// et calculer le P&L après sa confirmation.
#[derive(Debug, Clone)]
pub struct TransactionToConfirm {
    /// La signature de la transaction, utilisée pour interroger son statut.
    pub signature: Signature,
    /// Le coût estimé de la transaction en cas d'échec (tip Jito, frais de priorité, etc.).
    pub cost_on_failure: u64,
    /// L'heure à laquelle la transaction a été envoyée pour gérer le timeout.
    pub sent_at: Instant,
}

/// Service chargé de suivre les transactions envoyées, de vérifier leur statut
/// et de mettre à jour les métriques de P&L en conséquence.
pub struct ConfirmationService {
    /// Pour recevoir les signatures des transactions envoyées par le `UnifiedSender`.
    transaction_receiver: Receiver<TransactionToConfirm>,
    /// Client RPC pour interroger le statut des transactions.
    rpc_client: Arc<ResilientRpcClient>,
    /// La liste des transactions dont nous attendons la confirmation.
    pending_transactions: HashMap<Signature, TransactionToConfirm>,
}

impl ConfirmationService {
    /// Crée une nouvelle instance du service et un `Sender` pour lui envoyer des transactions.
    pub fn new(rpc_client: Arc<ResilientRpcClient>) -> (Self, Sender<TransactionToConfirm>) {
        let (tx_sender, tx_receiver) = mpsc::channel(1024);
        let service = Self {
            transaction_receiver: tx_receiver,
            rpc_client,
            pending_transactions: HashMap::new(),
        };
        (service, tx_sender)
    }

    /// Démarre la boucle principale du service dans une nouvelle tâche Tokio.
    pub fn start(mut self) -> JoinHandle<()> {
        tokio::spawn(async move {
            info!("[ConfirmationService] Démarrage du service de suivi de P&L.");
            self.run().await;
        })
    }

    /// La boucle principale qui gère la réception et la vérification des transactions.
    async fn run(&mut self) {
        // On attend un peu avant la première vérification pour laisser le temps aux tx de se propager.
        let mut check_interval = tokio::time::interval(Duration::from_secs(3));

        loop {
            tokio::select! {
                // Branche 1 : Une nouvelle transaction est arrivée du Sender
                Some(tx_to_confirm) = self.transaction_receiver.recv() => {
                    info!(signature = %tx_to_confirm.signature, "[P&L] Nouvelle transaction à suivre.");
                    self.pending_transactions.insert(tx_to_confirm.signature, tx_to_confirm);
                },

                // Branche 2 : Le timer de vérification s'est déclenché (uniquement si on a qqch à vérifier)
                _ = check_interval.tick(), if !self.pending_transactions.is_empty() => {
                    if let Err(e) = self.check_pending_transactions().await {
                        error!("[P&L] Erreur lors de la vérification des transactions: {}", e);
                    }
                }
            }
        }
    }

    /// Interroge le statut de toutes les transactions en attente et met à jour le P&L.
    async fn check_pending_transactions(&mut self) -> Result<()> {
        let signatures: Vec<Signature> = self.pending_transactions.keys().cloned().collect();
        let statuses = self.rpc_client.get_signature_statuses(&signatures).await
            .context("Échec de l'appel RPC getSignatureStatuses")?;

        let now = Instant::now();
        let mut signatures_to_remove = Vec::new();

        for (i, status_opt) in statuses.value.into_iter().enumerate() {
            let signature = signatures[i];
            let tx_info = self.pending_transactions.get(&signature).unwrap(); // On sait qu'il existe

            if let Some(status) = status_opt {
                // --- La transaction est confirmée ---
                if let Some(err) = status.err {
                    // Cas A: Transaction ÉCHOUÉE
                    warn!(signature = %signature, error = ?err, "[P&L] Transaction échouée.");
                    metrics::PNL_CUMULATIVE_LAMPORTS.sub(tx_info.cost_on_failure as i64);
                } else {
                    // Cas B: Transaction RÉUSSIE
                    info!(signature = %signature, "[P&L] Transaction réussie !");
                    match self.rpc_client.get_transaction(&signature, None).await {
                        Ok(tx_details) => {
                            if let Some(meta) = tx_details.transaction.meta {
                                // Étape 1: On extrait la valeur de l'OptionSerializer en utilisant un match.
                                // C'est la manière la plus sûre de gérer ce type spécifique.
                                let logs_option: Option<Vec<String>> = meta.log_messages.into();

                                // Étape 2: Maintenant que nous avons un vrai Option<Vec<String>>, on peut l'utiliser normalement.
                                if let Some(logs) = logs_option {
                                    if let Some(profit) = parse_realized_profit_from_logs(&logs) {
                                        info!(profit, "[P&L] Profit réel trouvé dans les logs.");
                                        metrics::PNL_CUMULATIVE_LAMPORTS.add(profit as i64);
                                    } else {
                                        warn!(signature = %signature, "[P&L] Transaction réussie mais profit non trouvé dans les logs.");
                                    }
                                }
                            }
                        },
                        Err(e) => error!(signature = %signature, error = %e, "[P&L] Impossible de récupérer les détails de la tx réussie."),
                    }
                }
                // On marque la signature pour suppression car elle est finalisée.
                signatures_to_remove.push(signature);

            } else {
                // --- La transaction n'est pas encore confirmée ---
                // Cas C: Vérifier si elle a expiré (timeout)
                if now.duration_since(tx_info.sent_at).as_secs() > TRANSACTION_TIMEOUT_SECS {
                    warn!(signature = %signature, "[P&L] Transaction considérée comme perdue (timeout).");
                    metrics::PNL_CUMULATIVE_LAMPORTS.sub(tx_info.cost_on_failure as i64);
                    signatures_to_remove.push(signature);
                }
            }
        }

        // On nettoie la liste des transactions en attente.
        for sig in signatures_to_remove {
            self.pending_transactions.remove(&sig);
        }

        Ok(())
    }
}