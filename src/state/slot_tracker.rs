use crate::rpc::ResilientRpcClient;
use anyhow::Result;
use arc_swap::ArcSwap;
use solana_sdk::sysvar::clock::{self, Clock};
use std::{
    sync::Arc,
    time::Instant,
};
use tracing::{error, info, warn, debug};

#[derive(Debug, Clone)]
pub struct SlotState {
    pub clock: Clock,
    pub received_at: Instant,
}

#[derive(Clone)]
pub struct SlotTracker {
    pub latest_slot_state: Arc<ArcSwap<SlotState>>,
}

impl SlotTracker {
    pub async fn new(rpc_client: &ResilientRpcClient) -> Result<Self> {
        println!("[SlotTracker] Initialisation : récupération du premier Clock...");
        let clock_account = rpc_client.get_account(&clock::ID).await?;
        let initial_clock: Clock = bincode::deserialize(&clock_account.data)?;
        info!(slot = initial_clock.slot, timestamp = initial_clock.unix_timestamp, "Initialisation du SlotTracker réussie.");

        let initial_state = SlotState {
            clock: initial_clock,
            received_at: Instant::now(),
        };

        Ok(Self {
            latest_slot_state: Arc::new(ArcSwap::from_pointee(initial_state)),
        })
    }

    pub fn update_from_geyser(&self, new_state: SlotState) {
        // On met à jour l'état seulement si le slot est plus récent
        if new_state.clock.slot > self.latest_slot_state.load().clock.slot {
            self.latest_slot_state.store(Arc::new(new_state));
        }
    }

    pub fn current(&self) -> Arc<SlotState> {
        self.latest_slot_state.load_full()
    }
}