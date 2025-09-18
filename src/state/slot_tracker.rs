// DANS : src/state/slot_tracker.rs
// REMPLACEZ L'INTÉGRALITÉ DU FICHIER PAR CECI :

use crate::rpc::ResilientRpcClient;
use anyhow::{anyhow, Result};
use arc_swap::ArcSwap;
use futures_util::SinkExt;
use solana_sdk::sysvar::clock::{self, Clock};
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::task::JoinHandle;
use tokio_stream::StreamExt;
use yellowstone_grpc_client::GeyserGrpcClient;
use yellowstone_grpc_proto::{
    prelude::{
        subscribe_update::UpdateOneof, CommitmentLevel, SubscribeRequest,
        SubscribeRequestFilterSlots,
    },
};

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
        println!("[SlotTracker] Initialisation réussie. Slot: {}, Timestamp: {}", initial_clock.slot, initial_clock.unix_timestamp);

        let initial_state = SlotState {
            clock: initial_clock,
            received_at: Instant::now(),
        };

        Ok(Self {
            latest_slot_state: Arc::new(ArcSwap::from_pointee(initial_state)),
        })
    }

    pub fn start(&self, geyser_grpc_url: String, rpc_client: Arc<ResilientRpcClient>) -> JoinHandle<()> {
        println!("[SlotTracker] Démarrage de la surveillance des slots via Geyser gRPC.");
        let latest_slot_clone = self.latest_slot_state.clone();

        tokio::spawn(async move {
            loop {
                let url = geyser_grpc_url.clone();
                let rpc = rpc_client.clone();
                let state_clone = latest_slot_clone.clone();

                println!("[SlotTracker] Tentative de connexion à Geyser : {}", url);
                match Self::slot_subscribe_loop(&url, rpc, state_clone).await {
                    Ok(_) => eprintln!("[SlotTracker] Stream terminé, reconnexion..."),
                    Err(e) => eprintln!("[SlotTracker] Erreur Geyser: {:?}. Reconnexion dans 5s...", e),
                }
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        })
    }

    pub fn current(&self) -> Arc<SlotState> {
        self.latest_slot_state.load_full()
    }

    async fn slot_subscribe_loop(
        geyser_grpc_url: &str,
        rpc_client: Arc<ResilientRpcClient>,
        latest_slot_state: Arc<ArcSwap<SlotState>>,
    ) -> Result<()> {
        let mut client = GeyserGrpcClient::build_from_shared(geyser_grpc_url.to_string())?
            .connect()
            .await?;
        let (mut subscribe_tx, mut stream) = client.subscribe().await?;

        // On peut laisser interslot_updates à true, ça ne coûte rien.
        // La logique ne traitera que les messages de fin de slot.
        let mut slot_filter = HashMap::new();
        slot_filter.insert(
            "slots".to_string(),
            SubscribeRequestFilterSlots {
                filter_by_commitment: Some(true),
                interslot_updates: Some(false),
            },
        );
        let request = SubscribeRequest {
            slots: slot_filter,
            commitment: Some(CommitmentLevel::Processed as i32),
            ..Default::default()
        };

        subscribe_tx.send(request).await?;
        println!("[SlotTracker] Abonnement au flux de slots Geyser réussi.");

        while let Some(message_result) = stream.next().await {
            let message = message_result.map_err(|e| anyhow!("Erreur de stream gRPC: {}", e))?;

            if let Some(UpdateOneof::Slot(slot_update)) = message.update_oneof {
                if slot_update.status == (CommitmentLevel::Processed as i32) {
                    let rpc_clone = rpc_client.clone();
                    let state_clone = latest_slot_state.clone();
                    let current_slot_on_chain = slot_update.slot;

                    tokio::spawn(async move {
                        if current_slot_on_chain > state_clone.load().clock.slot {
                            match rpc_clone.get_account(&clock::ID).await {
                                Ok(clock_account) => {
                                    if let Ok(new_clock) = bincode::deserialize::<Clock>(&clock_account.data) {
                                        let new_state = SlotState {
                                            clock: new_clock,
                                            received_at: Instant::now(),
                                        };
                                        state_clone.store(Arc::new(new_state));
                                    }
                                },
                                Err(e) => eprintln!("[SlotTracker] Avertissement: Échec du fetch de la Clock pour le slot {}: {}", current_slot_on_chain, e),
                            }
                        }
                    });
                }
            }
        }
        Err(anyhow!("Stream Geyser pour les slots terminé de manière inattendue."))
    }
}