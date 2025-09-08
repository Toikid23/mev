use anyhow::{Context, Result};
use solana_client::{client_error::{ClientError, ClientErrorKind}, nonblocking::rpc_client::RpcClient, rpc_config, rpc_response::{Response as RpcResponse, RpcSimulateTransactionResult}};
use solana_sdk::{
    account::Account, commitment_config::CommitmentConfig, pubkey::Pubkey,
    signature::Signature, transaction::{VersionedTransaction},

};
use std::{sync::Arc, time::Duration};
use tokio::time::sleep;
use rpc_config::RpcSimulateTransactionConfig;

/// Un "wrapper" autour du RpcClient de Solana qui ajoute une logique de
/// ré-essai automatique pour les appels RPC qui échouent à cause d'erreurs réseau temporaires.
#[derive(Clone)]
pub struct ResilientRpcClient {
    client: Arc<RpcClient>,
    max_retries: u8,
    delay_ms: u64,
}

impl ResilientRpcClient {
    /// Construit un nouveau client RPC résilient.
    pub fn new(rpc_url: String, max_retries: u8, delay_ms: u64) -> Self {
        Self {
            client: Arc::new(RpcClient::new(rpc_url)),
            max_retries,
            delay_ms,
        }
    }

    /// Méthode "passe-plat" pour accéder à la configuration de commitment du client sous-jacent.
    pub fn commitment(&self) -> CommitmentConfig {
        self.client.commitment()
    }

    /// Détermine si une erreur du client est temporaire et si une nouvelle tentative doit être effectuée.
    fn is_retryable(error: &ClientError) -> bool {
        matches!(
            error.kind,
            ClientErrorKind::Reqwest(_) | ClientErrorKind::RpcError(_) | ClientErrorKind::Io(_)
        )
    }

    // --- MÉTHODES WRAPPÉES AVEC LOGIQUE DE RÉ-ESSAI ---

    /// Récupère les données brutes d'un compte.
    pub async fn get_account_data(&self, pubkey: &Pubkey) -> Result<Vec<u8>> {
        for attempt in 0..=self.max_retries {
            match self.client.get_account_data(pubkey).await {
                Ok(data) => return Ok(data),
                Err(e) => {
                    if Self::is_retryable(&e) && attempt < self.max_retries {
                        // Log de l'erreur et de la nouvelle tentative
                        sleep(Duration::from_millis(self.delay_ms)).await;
                    } else {
                        return Err(e).with_context(|| format!("Échec final de get_account_data pour {}", pubkey));
                    }
                }
            }
        }
        unreachable!()
    }

    /// Récupère un compte complet.
    pub async fn get_account(&self, pubkey: &Pubkey) -> Result<Account> {
        for attempt in 0..=self.max_retries {
            match self.client.get_account(pubkey).await {
                Ok(account) => return Ok(account),
                Err(e) => {
                    if Self::is_retryable(&e) && attempt < self.max_retries {
                        sleep(Duration::from_millis(self.delay_ms)).await;
                    } else {
                        return Err(e).with_context(|| format!("Échec final de get_account pour {}", pubkey));
                    }
                }
            }
        }
        unreachable!()
    }

    /// Récupère plusieurs comptes.
    pub async fn get_multiple_accounts(&self, pubkeys: &[Pubkey]) -> Result<Vec<Option<Account>>> {
        for attempt in 0..=self.max_retries {
            match self.client.get_multiple_accounts(pubkeys).await {
                Ok(accounts) => return Ok(accounts),
                Err(e) => {
                    if Self::is_retryable(&e) && attempt < self.max_retries {
                        sleep(Duration::from_millis(self.delay_ms)).await;
                    } else {
                        return Err(e).with_context(|| "Échec final de get_multiple_accounts");
                    }
                }
            }
        }
        unreachable!()
    }

    /// Récupère le dernier blockhash.
    pub async fn get_latest_blockhash(&self) -> Result<solana_sdk::hash::Hash> {
        for attempt in 0..=self.max_retries {
            match self.client.get_latest_blockhash().await {
                Ok(hash) => return Ok(hash),
                Err(e) => {
                    if Self::is_retryable(&e) && attempt < self.max_retries {
                        sleep(Duration::from_millis(self.delay_ms)).await;
                    } else {
                        return Err(e).with_context(|| "Échec final de get_latest_blockhash");
                    }
                }
            }
        }
        unreachable!()
    }

    /// Envoie et confirme une transaction.
    pub async fn send_and_confirm_transaction(&self, transaction: &VersionedTransaction) -> Result<Signature> {
        for attempt in 0..=self.max_retries {
            match self.client.send_and_confirm_transaction(transaction).await {
                Ok(signature) => return Ok(signature),
                Err(e) => {
                    if Self::is_retryable(&e) && attempt < self.max_retries {
                        sleep(Duration::from_millis(self.delay_ms)).await;
                    } else {
                        return Err(e).with_context(|| "Échec final de send_and_confirm_transaction");
                    }
                }
            }
        }
        unreachable!()
    }

    /// Simule une transaction.
    pub async fn simulate_transaction(&self, transaction: &VersionedTransaction) -> Result<RpcResponse<RpcSimulateTransactionResult>> {
        for attempt in 0..=self.max_retries {
            match self.client.simulate_transaction(transaction).await {
                Ok(response) => return Ok(response),
                Err(e) => {
                    if Self::is_retryable(&e) && attempt < self.max_retries {
                        sleep(Duration::from_millis(self.delay_ms)).await;
                    } else {
                        return Err(e).with_context(|| "Échec final de simulate_transaction");
                    }
                }
            }
        }
        unreachable!()
    }

    pub async fn get_program_accounts_with_config(
        &self,
        program_id: &Pubkey,
        config: solana_client::rpc_config::RpcProgramAccountsConfig,
    ) -> Result<Vec<(Pubkey, Account)>> {
        for attempt in 0..=self.max_retries {
            match self
                .client
                .get_program_accounts_with_config(program_id, config.clone())
                .await
            {
                Ok(accounts) => return Ok(accounts),
                Err(e) => {
                    if Self::is_retryable(&e) && attempt < self.max_retries {
                        sleep(Duration::from_millis(self.delay_ms)).await;
                    } else {
                        return Err(e).with_context(|| {
                            format!(
                                "Échec final de get_program_accounts_with_config pour le programme {}",
                                program_id
                            )
                        });
                    }
                }
            }
        }
        unreachable!()
    }

    pub async fn simulate_transaction_with_config(
        &self,
        transaction: &VersionedTransaction,
        config: RpcSimulateTransactionConfig,
    ) -> Result<RpcResponse<RpcSimulateTransactionResult>> {
        for attempt in 0..=self.max_retries {
            match self
                .client
                .simulate_transaction_with_config(transaction, config.clone())
                .await
            {
                Ok(response) => return Ok(response),
                Err(e) => {
                    if Self::is_retryable(&e) && attempt < self.max_retries {
                        sleep(Duration::from_millis(self.delay_ms)).await;
                    } else {
                        return Err(e)
                            .with_context(|| "Échec final de simulate_transaction_with_config");
                    }
                }
            }
        }
        unreachable!()
    }

    pub async fn get_recent_prioritization_fees(
        &self,
        accounts: &[Pubkey],
    ) -> Result<Vec<solana_client::rpc_response::RpcPrioritizationFee>> {
        for attempt in 0..=self.max_retries {
            match self.client.get_recent_prioritization_fees(accounts).await {
                Ok(fees) => return Ok(fees),
                Err(e) => {
                    if Self::is_retryable(&e) && attempt < self.max_retries {
                        sleep(Duration::from_millis(self.delay_ms)).await;
                    } else {
                        return Err(e)
                            .with_context(|| "Échec final de get_recent_prioritization_fees");
                    }
                }
            }
        }
        unreachable!()
    }
}

