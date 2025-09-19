// DANS : src/bin/wallet_tracker.rs (VERSION FINALE POUR COPY-TRADING D'ARBITRAGE)

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

use anyhow::Result;
use solana_sdk::pubkey::Pubkey;
use std::{collections::{HashSet, BTreeSet}, fs, time::{Duration, Instant}, str::FromStr};
use tokio::sync::Mutex;
use tracing::{error, info, warn};
use zmq;
use mev::{
    communication::{GeyserUpdate, ZmqTopic},
    config::Config,
    filtering::cache::PoolCache,
};

const HOTLIST_FILE_NAME: &str = "copytrade_hotlist.json";

lazy_static::lazy_static! {
    static ref HOTLIST_FILE_LOCK: Mutex<()> = Mutex::new(());
}

async fn run_tracker() -> Result<()> {
    info!("[WalletTracker] Démarrage du service de Copy-Trading d'ARBITRAGE.");
    let config = Config::load()?;
    let tracked_wallets: HashSet<Pubkey> = config.copy_trade_wallets
        .into_iter()
        .filter_map(|s| Pubkey::from_str(&s).ok())
        .collect();

    if tracked_wallets.is_empty() {
        warn!("[WalletTracker] Aucune adresse n'est configurée. Le service ne fera rien.");
        std::future::pending::<()>().await;
        return Ok(());
    }
    info!("[WalletTracker] Surveillance de {} portefeuilles d'arbitrage.", tracked_wallets.len());

    let cache = PoolCache::load()?;
    info!("[WalletTracker] L'univers de {} pools a été chargé pour l'identification.", cache.pools.len());

    let context = zmq::Context::new();
    let subscriber = context.socket(zmq::SUB)?;
    subscriber.connect(mev::communication::ZMQ_DATA_ENDPOINT)?;
    subscriber.set_subscribe(&bincode::serialize(&ZmqTopic::Transaction)?)?;

    let mut last_write = Instant::now();
    let mut pending_pools = BTreeSet::new();

    loop {
        let multipart = subscriber.recv_multipart(0)?;
        if multipart.len() != 2 { continue; }

        if let Ok(GeyserUpdate::Transaction(tx_update)) = bincode::deserialize(&multipart[1]) {
            if let Some(signer) = tx_update.account_keys.first() {
                if tracked_wallets.contains(signer) {

                    // --- NOUVELLE LOGIQUE D'IDENTIFICATION D'ARBITRAGE ---
                    let mut pools_in_tx = HashSet::new();
                    for key in &tx_update.account_keys {
                        if let Some(pool_address) = cache.watch_map.get(key) {
                            pools_in_tx.insert(*pool_address);
                        }
                    }

                    // On considère que c'est un arbitrage uniquement si AU MOINS DEUX pools sont impliqués
                    if pools_in_tx.len() >= 2 {
                        info!(
                            "[WalletTracker] 🎯 HIT! Transaction d'arbitrage potentielle détectée par {}. Pools impliqués: {:?}",
                            signer, pools_in_tx
                        );
                        // On ajoute tous les pools de l'arbitrage détecté à la liste d'attente
                        for pool in pools_in_tx {
                            pending_pools.insert(pool);
                        }
                    }
                    // --- FIN DE LA NOUVELLE LOGIQUE ---
                }
            }
        }

        if !pending_pools.is_empty() && last_write.elapsed() > Duration::from_secs(2) {
            update_hotlist(&mut pending_pools).await?;
            last_write = Instant::now();
        }
    }
}

// La fonction update_hotlist reste la même.
async fn update_hotlist(pending_pools: &mut BTreeSet<Pubkey>) -> Result<()> {
    let _lock = HOTLIST_FILE_LOCK.lock().await;
    let mut hotlist: HashSet<Pubkey> = match fs::read_to_string(HOTLIST_FILE_NAME) {
        Ok(data) => serde_json::from_str(&data).unwrap_or_default(),
        Err(_) => HashSet::new(),
    };
    let mut new_additions = 0;
    for pool in pending_pools.iter() {
        if hotlist.insert(*pool) {
            new_additions += 1;
        }
    }
    if new_additions > 0 {
        info!("[WalletTracker] Ajout de {} nouveau(x) pool(s) à la hotlist de copy-trading.", new_additions);
        fs::write(HOTLIST_FILE_NAME, serde_json::to_string_pretty(&hotlist)?)?;
    }
    pending_pools.clear();
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    mev::monitoring::logging::setup_logging();
    dotenvy::dotenv().ok();
    if let Err(e) = run_tracker().await {
        error!("[WalletTracker] Le service a planté : {:?}.", e);
    }
    Ok(())
}