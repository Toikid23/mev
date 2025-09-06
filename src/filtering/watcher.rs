// DANS : src/filtering/watcher.rs

use super::cache::PoolCache;
use anyhow::{Context, Result};
use solana_sdk::pubkey::Pubkey;
use std::collections::{HashMap, HashSet};
use tokio::sync::mpsc;
use tokio_stream::StreamExt;
use futures_util::sink::SinkExt;

// --- Imports corrigés pour Yellowstone v9.0.0 ---
use yellowstone_grpc_client::GeyserGrpcClient;
use yellowstone_grpc_proto::{
    prelude::{
        subscribe_update::UpdateOneof, CommitmentLevel, SubscribeRequest,
        SubscribeRequestFilterTransactions,
    },
    tonic::transport::Channel,
};

/// Le Guetteur est responsable de la connexion au stream Geyser
/// et de l'identification des pools actifs en temps réel.
pub struct Watcher {
    geyser_grpc_url: String,
}

impl Watcher {
    pub fn new(geyser_grpc_url: String) -> Self {
        Self { geyser_grpc_url }
    }

    /// Lance le processus d'écoute.
    pub async fn run(
        &self,
        cache: PoolCache,
        active_pool_sender: mpsc::Sender<Pubkey>,
    ) -> Result<()> {
        println!("[Watcher] Connexion au stream Geyser gRPC à l'adresse : {}", self.geyser_grpc_url);

        // --- CORRECTION 1: Utilisation du Builder Pattern ---
        let mut client = GeyserGrpcClient::build_from_shared(self.geyser_grpc_url.clone())?
            .connect()
            .await
            .context("Impossible de se connecter au client Geyser gRPC")?;

        println!("[Watcher] Connexion réussie. Envoi de la requête d'abonnement...");

        // --- CORRECTION 2: Ajout des champs manquants au filtre ---
        let mut tx_filter = HashMap::new();
        tx_filter.insert(
            "txs".to_string(),
            SubscribeRequestFilterTransactions {
                vote: Some(false),
                failed: Some(false),
                account_include: vec![],
                account_required: vec![],
                account_exclude: vec![], // Champ manquant ajouté
                signature: None,         // Champ manquant ajouté
            },
        );

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

                        // Stratégie robuste : on vérifie tous les comptes de la transaction
                        for key_bytes in &tx_message.account_keys {
                            // --- CORRECTION 2: Conversion Vec<u8> -> Pubkey correcte ---
                            if key_bytes.len() == 32 {
                                let key = Pubkey::new_from_array(key_bytes.as_slice().try_into()?);
                                if let Some(pool_address) = cache.watch_map.get(&key) {
                                    found_pools.insert(*pool_address);
                                }
                            }
                        }

                        for pool_address in found_pools {
                            if let Err(e) = active_pool_sender.send(pool_address).await {
                                eprintln!("[Watcher] Erreur: Le canal est fermé. Arrêt. Raison: {}", e);
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