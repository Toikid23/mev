// DANS : src/state/leader_schedule.rs

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

#[derive(Clone, Default)]
pub struct FullSchedule {
    /// Clé: slot, Valeur: identity_pubkey du leader
    pub slot_to_identity: HashMap<u64, Pubkey>,
    /// Clé: identity_pubkey, Valeur: vote_pubkey
    pub identity_to_vote: HashMap<Pubkey, Pubkey>,
}

#[derive(Clone)]
pub struct LeaderScheduleTracker {
    schedule: Arc<ArcSwap<FullSchedule>>,
    current_epoch: Arc<AtomicU64>,
    rpc_client: Arc<ResilientRpcClient>,
}

impl LeaderScheduleTracker {
    pub async fn new(rpc_client: Arc<ResilientRpcClient>) -> Result<Self> {
        println!("[LeaderSchedule] Initialisation...");
        let tracker = Self {
            schedule: Arc::new(ArcSwap::from_pointee(FullSchedule::default())),
            current_epoch: Arc::new(AtomicU64::new(0)),
            rpc_client,
        };
        tracker.refresh_schedule().await?;
        println!("[LeaderSchedule] Initialisation réussie.");
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
                        println!("[LeaderSchedule] Changement d'epoch détecté ({} -> {}). Rafraîchissement...", known_epoch, epoch_info.epoch);
                        if let Err(e) = self_clone.refresh_schedule().await {
                            eprintln!("[LeaderSchedule] Erreur lors du rafraîchissement : {:?}", e);
                        }
                    }
                }
            }
        })
    }

    async fn refresh_schedule(&self) -> Result<()> {
        let epoch_info: EpochInfo = self.rpc_client.get_epoch_info().await?;
        let current_epoch = epoch_info.epoch;

        let (schedule_res, vote_accounts_res) = tokio::join!(
            self.rpc_client.get_leader_schedule(None),
            self.rpc_client.get_vote_accounts()
        );

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

        let identity_to_vote = {
            let vote_accounts = vote_accounts_res.context("Impossible de récupérer les vote accounts")?;
            let mut map = HashMap::new();
            for account in vote_accounts.current.iter().chain(vote_accounts.delinquent.iter()) {
                if let (Ok(identity), Ok(vote_pubkey)) = (
                    Pubkey::from_str(&account.node_pubkey),
                    Pubkey::from_str(&account.vote_pubkey),
                ) {
                    map.insert(identity, vote_pubkey);
                }
            }
            map
        };

        println!("[LeaderSchedule] Planning rafraîchi pour l'epoch {}. {} leaders mappés.", current_epoch, identity_to_vote.len());

        self.schedule.store(Arc::new(FullSchedule {
            slot_to_identity,
            identity_to_vote,
        }));
        self.current_epoch.store(current_epoch, Ordering::Relaxed);
        Ok(())
    }

    pub fn get_leader_info_for_slot(&self, slot: u64) -> Option<(Pubkey, Pubkey)> {
        let schedule = self.schedule.load();
        schedule.slot_to_identity
            .get(&slot)
            .and_then(|identity| {
                schedule.identity_to_vote.get(identity).map(|vote| (*identity, *vote))
            })
    }
}