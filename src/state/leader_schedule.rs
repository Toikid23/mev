// DANS : src/state/leader_schedule.rs (Version simplifiée)

use crate::rpc::ResilientRpcClient;
use anyhow::{Context, Result};
use arc_swap::ArcSwap;
use solana_sdk::{pubkey::Pubkey, epoch_info::EpochInfo};
use std::{
    collections::HashMap,
    str::FromStr,
    sync::{atomic::{AtomicU64, Ordering}, Arc},
    time::Duration,
};
use tokio::task::JoinHandle;
use tracing::{error, info, warn, debug};

/// Le service qui maintient le planning des leaders à jour.
#[derive(Clone)]
pub struct LeaderScheduleTracker {
    /// Clé: slot, Valeur: identity_pubkey du leader
    slot_to_identity: Arc<ArcSwap<HashMap<u64, Pubkey>>>,
    current_epoch: Arc<AtomicU64>,
    rpc_client: Arc<ResilientRpcClient>,
}

impl LeaderScheduleTracker {
    pub async fn new(rpc_client: Arc<ResilientRpcClient>) -> Result<Self> {
        info!("Initialisation du LeaderScheduleTracker...");
        let tracker = Self {
            slot_to_identity: Arc::new(ArcSwap::from_pointee(HashMap::new())),
            current_epoch: Arc::new(AtomicU64::new(0)),
            rpc_client,
        };
        tracker.refresh_schedule().await?;
        info!("Initialisation du LeaderScheduleTracker réussie.");
        Ok(tracker)
    }

    pub fn start(&self) -> JoinHandle<()> {
        let self_clone = self.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(60));
            loop {
                interval.tick().await;
                if let Ok(epoch_info) = self_clone.rpc_client.get_epoch_info().await {
                    let known_epoch = self_clone.current_epoch.load(Ordering::Relaxed);
                    if epoch_info.epoch > known_epoch {
                        info!(old_epoch = known_epoch, new_epoch = epoch_info.epoch, "Changement d'epoch détecté, rafraîchissement du planning.");
                        if let Err(e) = self_clone.refresh_schedule().await {
                            error!(error = ?e, "Erreur lors du rafraîchissement du planning des leaders.");
                        }
                    }
                }
            }
        })
    }

    /// Récupère le planning des leaders et le met en cache.
    async fn refresh_schedule(&self) -> Result<()> {
        let epoch_info: EpochInfo = self.rpc_client.get_epoch_info().await?;
        let current_epoch = epoch_info.epoch;

        // On ne fait plus qu'un seul appel RPC.
        let schedule_res = self.rpc_client.get_leader_schedule(None).await;

        let slot_to_identity = if let Some(schedule) = schedule_res.context("Impossible de récupérer le planning des leaders")? {
            let mut map = HashMap::new();
            for (leader_identity_str, slots) in &schedule {
                if let Ok(leader_identity) = Pubkey::from_str(leader_identity_str) {
                    for slot in slots {
                        map.insert(*slot as u64, leader_identity);
                    }
                }
            }
            map
        } else {
            HashMap::new()
        };

        info!(epoch = current_epoch, slot_count = slot_to_identity.len(), "Planning des leaders rafraîchi.");

        self.slot_to_identity.store(Arc::new(slot_to_identity));
        self.current_epoch.store(current_epoch, Ordering::Relaxed);
        Ok(())
    }

    /// Pour un slot donné, retourne l'identité du leader.
    pub fn get_leader_for_slot(&self, slot: u64) -> Option<Pubkey> {
        self.slot_to_identity.load().get(&slot).copied()
    }
}