// DANS : src/filtering/watcher.rs

use super::cache::PoolCache;
use anyhow::{Context, Result};
use solana_sdk::pubkey::Pubkey;
use std::collections::{HashMap, HashSet};
use tokio::sync::mpsc;
use tokio_stream::StreamExt;
use futures_util::sink::SinkExt;
use yellowstone_grpc_client::GeyserGrpcClient;
use yellowstone_grpc_proto::{
    prelude::{
        subscribe_update::UpdateOneof, CommitmentLevel, SubscribeRequest,
        SubscribeRequestFilterTransactions,
    },
};
use tracing::{error, info, warn, debug};

pub struct Watcher {
    geyser_grpc_url: String,
}

impl Watcher {
    pub fn new(geyser_grpc_url: String) -> Self {
        Self { geyser_grpc_url }
    }

    // --- LA SIGNATURE DE LA FONCTION CHANGE ---
    pub async fn run(
        &self,
        cache: PoolCache,
        programs_to_watch: Vec<Pubkey>, // <-- NOUVEL ARGUMENT
        active_pool_sender: mpsc::Sender<Pubkey>,
    ) -> Result<()> {
        info!(geyser_url = %self.geyser_grpc_url, "Connexion au stream Geyser gRPC.");


        let mut client = GeyserGrpcClient::build_from_shared(self.geyser_grpc_url.clone())?
            .connect()
            .await
            .context("Impossible de se connecter au client Geyser gRPC")?;

        println!("[Watcher] Connexion réussie. Préparation de l'abonnement pour {} programmes DEX...", programs_to_watch.len());

        // --- LA LOGIQUE DU FILTRE CHANGE ICI ---
        // On convertit les Pubkeys en String pour le filtre.
        let accounts_required_str: Vec<String> = programs_to_watch.into_iter().map(|p| p.to_string()).collect();

        let mut tx_filter = HashMap::new();
        tx_filter.insert(
            "txs".to_string(),
            SubscribeRequestFilterTransactions {
                vote: Some(false),
                failed: Some(false),
                account_include: vec![],
                // `account_required` signifie : "envoyez-moi uniquement les tx qui impliquent AU MOINS UN de ces comptes".
                // C'est parfait pour surveiller des programmes.
                account_required: accounts_required_str,
                account_exclude: vec![],
                signature: None,
            },
        );
        // --- FIN DE LA MODIFICATION DU FILTRE ---

        let request = SubscribeRequest {
            transactions: tx_filter,
            commitment: Some(CommitmentLevel::Processed as i32),
            ..Default::default()
        };

        let (mut subscribe_tx, mut stream) = client.subscribe().await?;
        subscribe_tx.send(request).await?;

        println!("[Watcher] Abonnement réussi. En attente du flux de transactions...");

        while let Some(message_result) = stream.next().await {
            let message = message_result.context("Erreur dans le stream Geyser")?;

            if let Some(UpdateOneof::Transaction(tx_update)) = message.update_oneof {
                if let Some(tx_info) = &tx_update.transaction {
                    if let Some(tx_message) = &tx_info.transaction.as_ref().unwrap().message {
                        let mut found_pools = HashSet::new();
                        for key_bytes in &tx_message.account_keys {
                            if key_bytes.len() == 32 {
                                let key = Pubkey::new_from_array(key_bytes.as_slice().try_into()?);
                                if let Some(pool_address) = cache.watch_map.get(&key) {
                                    found_pools.insert(*pool_address);
                                }
                            }
                        }
                        for pool_address in found_pools {
                            if let Err(e) = active_pool_sender.send(pool_address).await {
                                error!(error = %e, "Erreur: Le canal vers le filtering_engine est fermé. Arrêt du watcher.");
                                return Ok(());
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }
}