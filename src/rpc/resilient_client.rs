// DANS : src/rpc/resilient_client.rs

use anyhow::{Context, Result};
use solana_client::{client_error::{ClientError, ClientErrorKind}, nonblocking::rpc_client::RpcClient, rpc_config, rpc_response::{Response as RpcResponse, RpcSimulateTransactionResult}};
use solana_sdk::{
    account::Account, commitment_config::CommitmentConfig, pubkey::Pubkey,
    signature::Signature, transaction::{VersionedTransaction},
};
use solana_client::rpc_config::RpcTransactionConfig;
use solana_client::rpc_response::RpcConfirmedTransactionStatusWithSignature;
use solana_transaction_status::UiTransactionEncoding;
use std::{sync::Arc, time::{Duration, Instant}};
use tokio::time::sleep;
use rpc_config::RpcSimulateTransactionConfig;
use solana_sdk::epoch_info::EpochInfo;
use std::collections::HashMap;
use tracing::warn;
use crate::monitoring::metrics;

#[derive(Clone)]
pub struct ResilientRpcClient {
    client: Arc<RpcClient>,
    max_retries: u8,
    delay_ms: u64,
}

impl ResilientRpcClient {
    pub fn new(rpc_url: String, max_retries: u8, delay_ms: u64) -> Self {
        Self {
            client: Arc::new(RpcClient::new(rpc_url)),
            max_retries,
            delay_ms,
        }
    }

    pub fn commitment(&self) -> CommitmentConfig {
        self.client.commitment()
    }

    fn is_retryable(error: &ClientError) -> bool {
        matches!(
            error.kind,
            ClientErrorKind::Reqwest(_) | ClientErrorKind::RpcError(_) | ClientErrorKind::Io(_)
        )
    }

    pub async fn get_account_data(&self, pubkey: &Pubkey) -> Result<Vec<u8>> {
        const METHOD_NAME: &str = "get_account_data";
        for attempt in 0..=self.max_retries {
            let start_time = Instant::now();
            let result = self.client.get_account_data(pubkey).await;
            metrics::RPC_REQUEST_LATENCY.with_label_values(&[METHOD_NAME]).observe(start_time.elapsed().as_secs_f64());

            match result {
                Ok(data) => {
                    metrics::RPC_REQUESTS_TOTAL.with_label_values(&[METHOD_NAME, "success"]).inc();
                    return Ok(data);
                }
                Err(e) => {
                    metrics::RPC_REQUESTS_TOTAL.with_label_values(&[METHOD_NAME, "failure"]).inc();
                    if Self::is_retryable(&e) && attempt < self.max_retries {
                        warn!(attempt = attempt + 1, error = %e, pubkey = %pubkey, "Échec RPC (get_account_data), nouvelle tentative...");
                        sleep(Duration::from_millis(self.delay_ms)).await;
                    } else {
                        return Err(e).with_context(|| format!("Échec final de get_account_data pour {}", pubkey));
                    }
                }
            }
        }
        unreachable!()
    }

    pub async fn get_account(&self, pubkey: &Pubkey) -> Result<Account> {
        const METHOD_NAME: &str = "get_account";
        for attempt in 0..=self.max_retries {
            let start_time = Instant::now();
            let result = self.client.get_account(pubkey).await;
            metrics::RPC_REQUEST_LATENCY.with_label_values(&[METHOD_NAME]).observe(start_time.elapsed().as_secs_f64());

            match result {
                Ok(account) => {
                    metrics::RPC_REQUESTS_TOTAL.with_label_values(&[METHOD_NAME, "success"]).inc();
                    return Ok(account);
                }
                Err(e) => {
                    metrics::RPC_REQUESTS_TOTAL.with_label_values(&[METHOD_NAME, "failure"]).inc();
                    if Self::is_retryable(&e) && attempt < self.max_retries {
                        warn!(attempt = attempt + 1, error = %e, pubkey = %pubkey, "Échec RPC (get_account), nouvelle tentative...");
                        sleep(Duration::from_millis(self.delay_ms)).await;
                    } else {
                        return Err(e).with_context(|| format!("Échec final de get_account pour {}", pubkey));
                    }
                }
            }
        }
        unreachable!()
    }

    pub async fn get_multiple_accounts(&self, pubkeys: &[Pubkey]) -> Result<Vec<Option<Account>>> {
        const METHOD_NAME: &str = "get_multiple_accounts";
        for attempt in 0..=self.max_retries {
            let start_time = Instant::now();
            let result = self.client.get_multiple_accounts(pubkeys).await;
            metrics::RPC_REQUEST_LATENCY.with_label_values(&[METHOD_NAME]).observe(start_time.elapsed().as_secs_f64());

            match result {
                Ok(accounts) => {
                    metrics::RPC_REQUESTS_TOTAL.with_label_values(&[METHOD_NAME, "success"]).inc();
                    return Ok(accounts);
                }
                Err(e) => {
                    metrics::RPC_REQUESTS_TOTAL.with_label_values(&[METHOD_NAME, "failure"]).inc();
                    if Self::is_retryable(&e) && attempt < self.max_retries {
                        warn!(attempt = attempt + 1, error = %e, "Échec RPC (get_multiple_accounts), nouvelle tentative...");
                        sleep(Duration::from_millis(self.delay_ms)).await;
                    } else {
                        return Err(e).with_context(|| "Échec final de get_multiple_accounts");
                    }
                }
            }
        }
        unreachable!()
    }

    pub async fn get_latest_blockhash(&self) -> Result<solana_sdk::hash::Hash> {
        const METHOD_NAME: &str = "get_latest_blockhash";
        for attempt in 0..=self.max_retries {
            let start_time = Instant::now();
            let result = self.client.get_latest_blockhash().await;
            metrics::RPC_REQUEST_LATENCY.with_label_values(&[METHOD_NAME]).observe(start_time.elapsed().as_secs_f64());

            match result {
                Ok(hash) => {
                    metrics::RPC_REQUESTS_TOTAL.with_label_values(&[METHOD_NAME, "success"]).inc();
                    return Ok(hash);
                }
                Err(e) => {
                    metrics::RPC_REQUESTS_TOTAL.with_label_values(&[METHOD_NAME, "failure"]).inc();
                    if Self::is_retryable(&e) && attempt < self.max_retries {
                        warn!(attempt = attempt + 1, error = %e, "Échec RPC (get_latest_blockhash), nouvelle tentative...");
                        sleep(Duration::from_millis(self.delay_ms)).await;
                    } else {
                        return Err(e).with_context(|| "Échec final de get_latest_blockhash");
                    }
                }
            }
        }
        unreachable!()
    }

    pub async fn send_and_confirm_transaction(&self, transaction: &VersionedTransaction) -> Result<Signature> {
        const METHOD_NAME: &str = "send_and_confirm_transaction";
        for attempt in 0..=self.max_retries {
            let start_time = Instant::now();
            let result = self.client.send_and_confirm_transaction(transaction).await;
            metrics::RPC_REQUEST_LATENCY.with_label_values(&[METHOD_NAME]).observe(start_time.elapsed().as_secs_f64());

            match result {
                Ok(signature) => {
                    metrics::RPC_REQUESTS_TOTAL.with_label_values(&[METHOD_NAME, "success"]).inc();
                    return Ok(signature);
                }
                Err(e) => {
                    metrics::RPC_REQUESTS_TOTAL.with_label_values(&[METHOD_NAME, "failure"]).inc();
                    if Self::is_retryable(&e) && attempt < self.max_retries {
                        warn!(attempt = attempt + 1, error = %e, "Échec RPC (send_and_confirm_transaction), nouvelle tentative...");
                        sleep(Duration::from_millis(self.delay_ms)).await;
                    } else {
                        return Err(e).with_context(|| "Échec final de send_and_confirm_transaction");
                    }
                }
            }
        }
        unreachable!()
    }

    pub async fn simulate_transaction(&self, transaction: &VersionedTransaction) -> Result<RpcResponse<RpcSimulateTransactionResult>> {
        const METHOD_NAME: &str = "simulate_transaction";
        for attempt in 0..=self.max_retries {
            let start_time = Instant::now();
            let result = self.client.simulate_transaction(transaction).await;
            metrics::RPC_REQUEST_LATENCY.with_label_values(&[METHOD_NAME]).observe(start_time.elapsed().as_secs_f64());

            match result {
                Ok(response) => {
                    metrics::RPC_REQUESTS_TOTAL.with_label_values(&[METHOD_NAME, "success"]).inc();
                    return Ok(response);
                }
                Err(e) => {
                    metrics::RPC_REQUESTS_TOTAL.with_label_values(&[METHOD_NAME, "failure"]).inc();
                    if Self::is_retryable(&e) && attempt < self.max_retries {
                        warn!(attempt = attempt + 1, error = %e, "Échec RPC (simulate_transaction), nouvelle tentative...");
                        sleep(Duration::from_millis(self.delay_ms)).await;
                    } else {
                        return Err(e).with_context(|| "Échec final de simulate_transaction");
                    }
                }
            }
        }
        unreachable!()
    }

    pub async fn get_program_accounts_with_config(&self, program_id: &Pubkey, config: solana_client::rpc_config::RpcProgramAccountsConfig) -> Result<Vec<(Pubkey, Account)>> {
        const METHOD_NAME: &str = "get_program_accounts_with_config";
        for attempt in 0..=self.max_retries {
            let start_time = Instant::now();
            let result = self.client.get_program_accounts_with_config(program_id, config.clone()).await;
            metrics::RPC_REQUEST_LATENCY.with_label_values(&[METHOD_NAME]).observe(start_time.elapsed().as_secs_f64());

            match result {
                Ok(accounts) => {
                    metrics::RPC_REQUESTS_TOTAL.with_label_values(&[METHOD_NAME, "success"]).inc();
                    return Ok(accounts);
                }
                Err(e) => {
                    metrics::RPC_REQUESTS_TOTAL.with_label_values(&[METHOD_NAME, "failure"]).inc();
                    if Self::is_retryable(&e) && attempt < self.max_retries {
                        warn!(attempt = attempt + 1, error = %e, program_id = %program_id, "Échec RPC (get_program_accounts_with_config), nouvelle tentative...");
                        sleep(Duration::from_millis(self.delay_ms)).await;
                    } else {
                        return Err(e).with_context(|| format!("Échec final de get_program_accounts_with_config pour le programme {}", program_id));
                    }
                }
            }
        }
        unreachable!()
    }

    pub async fn simulate_transaction_with_config(&self, transaction: &VersionedTransaction, config: RpcSimulateTransactionConfig) -> Result<RpcResponse<RpcSimulateTransactionResult>> {
        const METHOD_NAME: &str = "simulate_transaction_with_config";
        for attempt in 0..=self.max_retries {
            let start_time = Instant::now();
            let result = self.client.simulate_transaction_with_config(transaction, config.clone()).await;
            metrics::RPC_REQUEST_LATENCY.with_label_values(&[METHOD_NAME]).observe(start_time.elapsed().as_secs_f64());

            match result {
                Ok(response) => {
                    metrics::RPC_REQUESTS_TOTAL.with_label_values(&[METHOD_NAME, "success"]).inc();
                    return Ok(response);
                }
                Err(e) => {
                    metrics::RPC_REQUESTS_TOTAL.with_label_values(&[METHOD_NAME, "failure"]).inc();
                    if Self::is_retryable(&e) && attempt < self.max_retries {
                        warn!(attempt = attempt + 1, error = %e, "Échec RPC (simulate_transaction_with_config), nouvelle tentative...");
                        sleep(Duration::from_millis(self.delay_ms)).await;
                    } else {
                        return Err(e).with_context(|| "Échec final de simulate_transaction_with_config");
                    }
                }
            }
        }
        unreachable!()
    }

    pub async fn get_recent_prioritization_fees(&self, accounts: &[Pubkey]) -> Result<Vec<solana_client::rpc_response::RpcPrioritizationFee>> {
        const METHOD_NAME: &str = "get_recent_prioritization_fees";
        for attempt in 0..=self.max_retries {
            let start_time = Instant::now();
            let result = self.client.get_recent_prioritization_fees(accounts).await;
            metrics::RPC_REQUEST_LATENCY.with_label_values(&[METHOD_NAME]).observe(start_time.elapsed().as_secs_f64());

            match result {
                Ok(fees) => {
                    metrics::RPC_REQUESTS_TOTAL.with_label_values(&[METHOD_NAME, "success"]).inc();
                    return Ok(fees);
                }
                Err(e) => {
                    metrics::RPC_REQUESTS_TOTAL.with_label_values(&[METHOD_NAME, "failure"]).inc();
                    if Self::is_retryable(&e) && attempt < self.max_retries {
                        warn!(attempt = attempt + 1, error = %e, "Échec RPC (get_recent_prioritization_fees), nouvelle tentative...");
                        sleep(Duration::from_millis(self.delay_ms)).await;
                    } else {
                        return Err(e).with_context(|| "Échec final de get_recent_prioritization_fees");
                    }
                }
            }
        }
        unreachable!()
    }

    pub async fn get_epoch_info(&self) -> Result<EpochInfo> {
        const METHOD_NAME: &str = "get_epoch_info";
        for attempt in 0..=self.max_retries {
            let start_time = Instant::now();
            let result = self.client.get_epoch_info().await;
            metrics::RPC_REQUEST_LATENCY.with_label_values(&[METHOD_NAME]).observe(start_time.elapsed().as_secs_f64());

            match result {
                Ok(info) => {
                    metrics::RPC_REQUESTS_TOTAL.with_label_values(&[METHOD_NAME, "success"]).inc();
                    return Ok(info);
                }
                Err(e) => {
                    metrics::RPC_REQUESTS_TOTAL.with_label_values(&[METHOD_NAME, "failure"]).inc();
                    if Self::is_retryable(&e) && attempt < self.max_retries {
                        warn!(attempt = attempt + 1, error = %e, "Échec RPC (get_epoch_info), nouvelle tentative...");
                        sleep(Duration::from_millis(self.delay_ms)).await;
                    } else {
                        return Err(e).with_context(|| "Échec final de get_epoch_info");
                    }
                }
            }
        }
        unreachable!()
    }

    pub async fn get_leader_schedule(&self, epoch: Option<u64>) -> Result<Option<HashMap<String, Vec<usize>>>> {
        const METHOD_NAME: &str = "get_leader_schedule";
        for attempt in 0..=self.max_retries {
            let start_time = Instant::now();
            let result = self.client.get_leader_schedule(epoch).await;
            metrics::RPC_REQUEST_LATENCY.with_label_values(&[METHOD_NAME]).observe(start_time.elapsed().as_secs_f64());

            match result {
                Ok(schedule) => {
                    metrics::RPC_REQUESTS_TOTAL.with_label_values(&[METHOD_NAME, "success"]).inc();
                    return Ok(schedule);
                }
                Err(e) => {
                    metrics::RPC_REQUESTS_TOTAL.with_label_values(&[METHOD_NAME, "failure"]).inc();
                    if Self::is_retryable(&e) && attempt < self.max_retries {
                        warn!(attempt = attempt + 1, error = %e, "Échec RPC (get_leader_schedule), nouvelle tentative...");
                        sleep(Duration::from_millis(self.delay_ms)).await;
                    } else {
                        return Err(e).with_context(|| "Échec final de get_leader_schedule");
                    }
                }
            }
        }
        unreachable!()
    }

    pub async fn get_vote_accounts(&self) -> Result<solana_client::rpc_response::RpcVoteAccountStatus> {
        const METHOD_NAME: &str = "get_vote_accounts";
        for attempt in 0..=self.max_retries {
            let start_time = Instant::now();
            let result = self.client.get_vote_accounts().await;
            metrics::RPC_REQUEST_LATENCY.with_label_values(&[METHOD_NAME]).observe(start_time.elapsed().as_secs_f64());

            match result {
                Ok(accounts) => {
                    metrics::RPC_REQUESTS_TOTAL.with_label_values(&[METHOD_NAME, "success"]).inc();
                    return Ok(accounts);
                }
                Err(e) => {
                    metrics::RPC_REQUESTS_TOTAL.with_label_values(&[METHOD_NAME, "failure"]).inc();
                    if Self::is_retryable(&e) && attempt < self.max_retries {
                        warn!(attempt = attempt + 1, error = %e, "Échec RPC (get_vote_accounts), nouvelle tentative...");
                        sleep(Duration::from_millis(self.delay_ms)).await;
                    } else {
                        return Err(e).with_context(|| "Échec final de get_vote_accounts");
                    }
                }
            }
        }
        unreachable!()
    }

    pub async fn get_signatures_for_address_with_limit(&self, address: &Pubkey, limit: usize) -> Result<Vec<RpcConfirmedTransactionStatusWithSignature>> {
        const METHOD_NAME: &str = "get_signatures_for_address";
        for attempt in 0..=self.max_retries {
            let start_time = Instant::now();
            let result = self.client.get_signatures_for_address(address).await;
            metrics::RPC_REQUEST_LATENCY.with_label_values(&[METHOD_NAME]).observe(start_time.elapsed().as_secs_f64());

            match result {
                Ok(mut signatures) => {
                    metrics::RPC_REQUESTS_TOTAL.with_label_values(&[METHOD_NAME, "success"]).inc();
                    signatures.truncate(limit);
                    return Ok(signatures);
                }
                Err(e) => {
                    metrics::RPC_REQUESTS_TOTAL.with_label_values(&[METHOD_NAME, "failure"]).inc();
                    if Self::is_retryable(&e) && attempt < self.max_retries {
                        warn!(attempt = attempt + 1, error = %e, address = %address, "Échec RPC (get_signatures_for_address), nouvelle tentative...");
                        sleep(Duration::from_millis(self.delay_ms)).await;
                    } else {
                        return Err(e).with_context(|| "Échec final de get_signatures_for_address");
                    }
                }
            }
        }
        unreachable!()
    }

    pub async fn get_transaction(&self, signature: &Signature, encoding: Option<UiTransactionEncoding>) -> Result<solana_transaction_status::EncodedConfirmedTransactionWithStatusMeta> {
        const METHOD_NAME: &str = "get_transaction";
        let config = RpcTransactionConfig {
            encoding,
            commitment: Some(self.commitment()),
            max_supported_transaction_version: Some(0),
        };

        for attempt in 0..=self.max_retries {
            let start_time = Instant::now();
            let result = self.client.get_transaction_with_config(signature, config).await;
            metrics::RPC_REQUEST_LATENCY.with_label_values(&[METHOD_NAME]).observe(start_time.elapsed().as_secs_f64());

            match result {
                Ok(tx) => {
                    metrics::RPC_REQUESTS_TOTAL.with_label_values(&[METHOD_NAME, "success"]).inc();
                    return Ok(tx);
                }
                Err(e) => {
                    metrics::RPC_REQUESTS_TOTAL.with_label_values(&[METHOD_NAME, "failure"]).inc();
                    if Self::is_retryable(&e) && attempt < self.max_retries {
                        warn!(attempt = attempt + 1, error = %e, signature = %signature, "Échec RPC (get_transaction), nouvelle tentative...");
                        sleep(Duration::from_millis(self.delay_ms)).await;
                    } else {
                        return Err(e).with_context(|| "Échec final de get_transaction");
                    }
                }
            }
        }
        unreachable!()
    }

    pub async fn get_slot(&self) -> Result<u64> {
        const METHOD_NAME: &str = "get_slot";
        for attempt in 0..=self.max_retries {
            let start_time = Instant::now();
            let result = self.client.get_slot().await;
            metrics::RPC_REQUEST_LATENCY.with_label_values(&[METHOD_NAME]).observe(start_time.elapsed().as_secs_f64());

            match result {
                Ok(slot) => {
                    metrics::RPC_REQUESTS_TOTAL.with_label_values(&[METHOD_NAME, "success"]).inc();
                    return Ok(slot);
                }
                Err(e) => {
                    metrics::RPC_REQUESTS_TOTAL.with_label_values(&[METHOD_NAME, "failure"]).inc();
                    if Self::is_retryable(&e) && attempt < self.max_retries {
                        warn!(attempt = attempt + 1, error = %e, "Échec RPC (get_slot), nouvelle tentative...");
                        sleep(Duration::from_millis(self.delay_ms)).await;
                    } else {
                        return Err(e).with_context(|| "Échec final de get_slot");
                    }
                }
            }
        }
        unreachable!()
    }
}