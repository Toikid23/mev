#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

use futures_util::{Sink, SinkExt}; // On a besoin de `Sink` et `SinkExt`
use std::error::Error; // Pour le type de l'erreur dans le Sink
use std::pin::Pin; // Pour pouvoir utiliser le Sink
use anyhow::{Context, Result};
use solana_sdk::pubkey::Pubkey;
use std::{collections::{HashMap, HashSet}, time::Duration};
use tokio_stream::StreamExt;
use tracing::{error, info};
use yellowstone_grpc_client::GeyserGrpcClient;
use yellowstone_grpc_proto::prelude::{
    subscribe_update::UpdateOneof, CommitmentLevel, SubscribeRequest,
    SubscribeRequestFilterAccounts, SubscribeRequestFilterSlots,
    SubscribeRequestFilterTransactions,
};
use zmq;
use mev::communication::{
    GatewayCommand, GeyserUpdate, ZmqTopic, ZMQ_COMMAND_ENDPOINT, ZMQ_DATA_ENDPOINT,
};

async fn resubscribe_to_geyser<S, E>(
    subscribe_tx: &mut S,
    watched_accounts: &HashSet<Pubkey>,
) -> Result<()>
where
    S: Sink<SubscribeRequest, Error = E> + Unpin,
    E: Error + Send + Sync + 'static,
{
    info!("[Gateway] Préparation d'une nouvelle requête d'abonnement Geyser...");

    // (Le corps de la fonction ne change absolument pas, il est déjà correct)
    let programs_to_watch: Vec<String> = vec![
        "675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8", "CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C",
        "CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK", "whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc",
        "Eo7WjKq67rjJQSZxS6z3YkapzY3eMj6Xy8X5EQVn5UaB", "cpamdpZCGKUy5JxQXB4dcpGPiikHawvSWAd6mEn1sGG",
        "LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo", "pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA",
    ].into_iter().map(String::from).collect();

    let request = SubscribeRequest {
        slots: HashMap::from([( "slots".to_string(), SubscribeRequestFilterSlots { filter_by_commitment: Some(true), interslot_updates: Some(false) })]),
        transactions: HashMap::from([( "txs".to_string(), SubscribeRequestFilterTransactions { vote: Some(false), failed: Some(false), account_include: vec![], account_required: programs_to_watch, account_exclude: vec![], signature: None })]),
        accounts: HashMap::from([( "accounts".to_string(), SubscribeRequestFilterAccounts { account: watched_accounts.iter().map(|p| p.to_string()).collect(), owner: vec![], filters: vec![], nonempty_txn_signature: None })]),
        commitment: Some(CommitmentLevel::Processed as i32),
        ..Default::default()
    };

    info!("[Gateway] Nouvel abonnement inclura {} comptes spécifiques.", watched_accounts.len());

    // On utilise la méthode `send` du trait `Sink`
    Pin::new(subscribe_tx).send(request).await.map_err(|e| anyhow::anyhow!(e))?;

    info!("[Gateway] Nouvelle requête d'abonnement envoyée.");
    Ok(())
}

async fn run_gateway() -> Result<()> {
    let context = zmq::Context::new();
    let publisher = context.socket(zmq::PUB)?;
    publisher.bind(ZMQ_DATA_ENDPOINT)?;
    let commander = context.socket(zmq::PULL)?;
    commander.bind(ZMQ_COMMAND_ENDPOINT)?;
    info!("[Gateway] Sockets ZMQ (PUB/PULL) prêts.");

    let geyser_url = std::env::var("GEYSER_GRPC_URL")?;
    let mut client = GeyserGrpcClient::build_from_shared(geyser_url)?.connect().await?;
    let (mut subscribe_tx, mut stream) = client.subscribe().await?;
    let mut watched_accounts: HashSet<Pubkey> = HashSet::new();
    resubscribe_to_geyser(&mut subscribe_tx, &watched_accounts).await?;

    loop {
        // On utilise `try_recv_multipart` dans une boucle `select!` pour ne pas bloquer.
        let command_future = async {
            match commander.recv_multipart(zmq::DONTWAIT) {
                Ok(msg) => Some(msg),
                Err(_) => None,
            }
        };

        tokio::select! {
            Some(message_result) = stream.next() => {
                let message = message_result.context("Erreur de stream gRPC")?;
                if let Some(update) = message.update_oneof {
                    let (topic, payload_result) = match update {
                        UpdateOneof::Slot(s) => (ZmqTopic::Slot, bincode::serialize(&GeyserUpdate::Slot(s.into()))),
                        UpdateOneof::Account(a) => (ZmqTopic::Account, bincode::serialize(&GeyserUpdate::Account(a.into()))),
                        UpdateOneof::Transaction(t) => (ZmqTopic::Transaction, bincode::serialize(&GeyserUpdate::Transaction(t.into()))),
                        _ => continue,
                    };
                    if let Ok(payload) = payload_result {
                        publisher.send_multipart(&[bincode::serialize(&topic)?, payload], 0)?;
                    }
                }
            },
            Some(command_msg) = command_future => {
                if let Some(payload) = command_msg.get(0) {
                    if let Ok(GatewayCommand::UpdateAccountSubscriptions(new_accounts)) = bincode::deserialize(payload) {
                        let new_accounts_set: HashSet<Pubkey> = new_accounts.into_iter().collect();
                        if new_accounts_set != watched_accounts {
                            info!("[Gateway] Commande reçue pour mettre à jour les abonnements.");
                            watched_accounts = new_accounts_set;
                            if let Err(e) = resubscribe_to_geyser(&mut subscribe_tx, &watched_accounts).await {
                                error!("[Gateway] Échec re-souscription: {}", e);
                            }
                        }
                    }
                }
            },
        }
    }
}
#[tokio::main]
async fn main() -> Result<()> {
    mev::monitoring::logging::setup_logging();
    dotenvy::dotenv().ok();
    info!("[Gateway] Démarrage du service Geyser Gateway (Mode Proactif).");
    loop {
        if let Err(e) = run_gateway().await {
            error!("[Gateway] Le service a planté : {:?}. Redémarrage dans 5s...", e);
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    }
}