// DANS : src/state/slot_tracker.rs (Version Complète et Finale)

use crate::rpc::ResilientRpcClient;
use anyhow::{Context, Result};
use arc_swap::ArcSwap;
use futures_util::StreamExt;
use solana_client::nonblocking::pubsub_client::PubsubClient;
use solana_sdk::sysvar::clock::{self, Clock};
use std::sync::Arc;
use tokio::task::JoinHandle;

/// Maintient une copie en mémoire du Sysvar Clock,
/// mise à jour en temps réel via un abonnement aux slots.
#[derive(Clone)]
pub struct SlotTracker {
    latest_clock: Arc<ArcSwap<Clock>>,
}

impl SlotTracker {
    /// Crée une nouvelle instance du tracker, initialisée avec une première version du Clock.
    pub async fn new(rpc_client: &ResilientRpcClient) -> Result<Self> {
        println!("[SlotTracker] Initialisation : récupération du premier Clock...");
        let initial_clock_account = rpc_client
            .get_account(&clock::ID)
            .await
            .context("Échec de la récupération initiale du Clock pour le SlotTracker")?;

        let initial_clock: Clock = bincode::deserialize(&initial_clock_account.data)?;

        println!("[SlotTracker] Initialisation réussie. Timestamp actuel : {}", initial_clock.unix_timestamp);

        Ok(Self {
            latest_clock: Arc::new(ArcSwap::from_pointee(initial_clock)),
        })
    }

    /// Démarre la tâche de fond qui écoute les mises à jour de slot et rafraîchit le Clock.
    pub fn start(
        &self,
        rpc_client: Arc<ResilientRpcClient>,
        pubsub_client: Arc<PubsubClient>,
    ) -> JoinHandle<()> {

        println!("[SlotTracker] Démarrage de la tâche de fond de surveillance des slots...");
        let clock_clone = self.latest_clock.clone();

        tokio::spawn(async move {
            if let Err(e) = Self::slot_subscribe_loop(rpc_client, pubsub_client, clock_clone).await {
                eprintln!("[SlotTracker] ERREUR CRITIQUE : La boucle de surveillance des slots s'est arrêtée : {:?}", e);
            }
        })
    }

    /// Retourne un instantané (`Arc`) du dernier Clock connu.
    pub fn current(&self) -> Arc<Clock> {
        self.latest_clock.load_full()
    }

    /// La boucle de la tâche de fond qui met à jour le Clock.
    async fn slot_subscribe_loop(
        rpc_client: Arc<ResilientRpcClient>,
        pubsub_client: Arc<PubsubClient>,
        latest_clock: Arc<ArcSwap<Clock>>,
    ) -> Result<()> {

        println!("[SlotTracker] Abonnement au flux de slots via WebSocket...");
        let (mut stream, _unsubscribe) = pubsub_client
            .slot_subscribe()
            .await
            .context("Échec de l'abonnement au flux de slots")?;

        println!("[SlotTracker] Abonnement réussi. En attente des mises à jour de slots...");

        while let Some(slot_info) = stream.next().await {
            let rpc_clone = rpc_client.clone();
            let clock_clone = latest_clock.clone();

            tokio::spawn(async move {
                match rpc_clone.get_account(&clock::ID).await {
                    Ok(clock_account) => {
                        if let Ok(new_clock) = bincode::deserialize::<Clock>(&clock_account.data) {
                            // On stocke la nouvelle version du Clock.
                            clock_clone.store(Arc::new(new_clock));
                            // Décommenter pour un log très verbeux
                            // println!("[SlotTracker] Clock mis à jour au slot {}. Timestamp: {}", slot_info.slot, new_clock.unix_timestamp);
                        }
                    },
                    Err(e) => {
                        eprintln!("[SlotTracker] AVERTISSEMENT : Échec de la récupération du Clock pour le slot {}: {:?}", slot_info.slot, e);
                    }
                }
            });
        }

        Err(anyhow::anyhow!("Le stream de slots WebSocket s'est terminé de manière inattendue."))
    }
}