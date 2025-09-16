// DANS : src/middleware/final_simulator.rs

use super::{ExecutionContext, Middleware};
use crate::execution::sender::{ArbitrageSendInfo, TransactionSender};
use crate::execution::simulator;
use crate::monitoring::metrics;
use anyhow::{Result, Ok};
use async_trait::async_trait;
use solana_client::rpc_config::RpcSimulateTransactionConfig;
use solana_transaction_status::UiTransactionEncoding;
use std::sync::Arc;
use tracing::{error, info, warn};

pub struct FinalSimulator {
    sender: Arc<dyn TransactionSender>,
}

impl FinalSimulator {
    pub fn new(sender: Arc<dyn TransactionSender>) -> Self {
        Self { sender }
    }
}

#[async_trait]
impl Middleware for FinalSimulator {
    fn name(&self) -> &'static str {
        "FinalSimulator"
    }

    async fn process(&self, context: &mut ExecutionContext) -> Result<bool> {
        let tx_to_simulate = match &context.final_tx {
            Some(tx) => tx,
            None => return Ok(false), // Arrêt propre si aucune transaction n'a été construite
        };

        let sim_config = RpcSimulateTransactionConfig {
            sig_verify: false,
            replace_recent_blockhash: true,
            commitment: Some(context.rpc_client.commitment()),
            encoding: Some(UiTransactionEncoding::Base64),
            accounts: None,
            min_context_slot: None,
            inner_instructions: false,
        };

        match context.rpc_client.simulate_transaction_with_config(tx_to_simulate, sim_config).await {
            Result::Ok(sim_response) if sim_response.value.err.is_none() => {
                info!("SUCCÈS DE LA SIMULATION FINALE ! Passage à l'envoi.");
                metrics::TRANSACTION_OUTCOMES.with_label_values(&["SimSuccess", &context.pool_pair_id]).inc();

                if let Some(logs) = &sim_response.value.logs {
                    if let Some(realized_profit) = simulator::parse_realized_profit_from_logs(logs) {
                        let slippage = (context.estimated_profit.unwrap() as i64) - (realized_profit as i64);
                        info!(realized_profit, slippage, "Analyse financière (simulation) terminée.");
                    }
                }

                let send_info = ArbitrageSendInfo {
                    transaction: tx_to_simulate.clone(),
                    is_jito_leader: context.is_jito_leader,
                    jito_tip: context.jito_tip,
                    estimated_profit: context.estimated_profit.unwrap(),
                };

                match self.sender.send_transaction(send_info).await {
                    Result::Ok(signature) => { // Utilisez Result::Ok
                        info!(signature = %signature, "Transaction envoyée avec succès !");
                        metrics::TRANSACTIONS_SENT.inc();
                        let outcome = if context.is_jito_leader { "Sent_Jito" } else { "Sent_Normal" };
                        context.span.record("decision", outcome);
                    }
                    Err(e) => {
                        error!(error = %e, "Échec de l'envoi de la transaction.");
                        let outcome = if context.is_jito_leader { "SendFailure_Jito" } else { "SendFailure_Normal" };
                        context.span.record("outcome", outcome);
                    }
                }
            }
            Result::Ok(sim_response) => { // Utilisez Result::Ok
                warn!(error = ?sim_response.value.err, "ÉCHEC DE LA SIMULATION FINALE.");
                if let Some(logs) = sim_response.value.logs { for log in logs { warn!(log = log); } }
                metrics::TRANSACTION_OUTCOMES.with_label_values(&["SimFailure", &context.pool_pair_id]).inc();
            }
            Err(e) => {
                error!(error = %e, "ERREUR RPC pendant la simulation finale.");
                metrics::TRANSACTION_OUTCOMES.with_label_values(&["RpcError", &context.pool_pair_id]).inc();
            }
        }

        Ok(false) // On arrête toujours le pipeline ici, que ce soit un succès ou un échec
    }
}