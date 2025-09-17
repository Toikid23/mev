// DANS : src/state/balance_tracker.rs

use crate::config::Config;
use crate::rpc::ResilientRpcClient;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use solana_sdk::signer::Signer;
use std::{
    fs::{File},
    io::{BufReader, BufWriter},
    path::Path,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{sync::Mutex, task::JoinHandle};
use tracing::{info, warn};

const HISTORY_FILE_NAME: &str = "balance_history.json";
const MONITORING_INTERVAL_SECS: u64 = 900; // 15 minutes
pub const HISTORY_WINDOW_SECS: u64 = 86400; // 24 heures

lazy_static::lazy_static! {
    static ref HISTORY_FILE_LOCK: Mutex<()> = Mutex::new(());
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
pub struct BalanceEntry {
    pub timestamp: u64,
    pub lamports: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct BalanceHistory {
    pub entries: Vec<BalanceEntry>,
}

impl BalanceHistory {
    pub async fn load() -> Result<Self> {
        let _lock = HISTORY_FILE_LOCK.lock().await;
        if !Path::new(HISTORY_FILE_NAME).exists() {
            return Ok(Self::default());
        }
        let file = File::open(HISTORY_FILE_NAME)?;
        let reader = BufReader::new(file);
        serde_json::from_reader(reader).context("Failed to deserialize balance history")
    }

    pub async fn save(&self) -> Result<()> {
        let _lock = HISTORY_FILE_LOCK.lock().await;
        let file = File::create(HISTORY_FILE_NAME)?;
        let writer = BufWriter::new(file);
        serde_json::to_writer_pretty(writer, self).context("Failed to serialize balance history")
    }
}

pub fn start_monitoring(rpc_client: Arc<ResilientRpcClient>, config: Config) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(MONITORING_INTERVAL_SECS));
        let payer_pubkey = config.payer_keypair().unwrap().pubkey();

        loop {
            interval.tick().await;
            info!("[BalanceTracker] Enregistrement du solde actuel...");

            match rpc_client.get_account(&payer_pubkey).await {
                Ok(account) => {
                    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
                    let new_entry = BalanceEntry { timestamp: now, lamports: account.lamports };

                    match BalanceHistory::load().await {
                        Ok(mut history) => {
                            history.entries.push(new_entry);
                            // On ne garde que les données de la fenêtre glissante
                            history.entries.retain(|e| now.saturating_sub(e.timestamp) < HISTORY_WINDOW_SECS);
                            if let Err(e) = history.save().await {
                                warn!("[BalanceTracker] Erreur lors de la sauvegarde de l'historique : {}", e);
                            }
                        }
                        Err(e) => warn!("[BalanceTracker] Erreur lors du chargement de l'historique : {}", e),
                    }
                }
                Err(e) => warn!("[BalanceTracker] Impossible de récupérer le solde du portefeuille : {}", e),
            }
        }
    })
}