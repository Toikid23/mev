
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

use anyhow::{Result};
use solana_sdk::pubkey::Pubkey;
use std::{collections::{HashMap, HashSet}, fs, time::{Duration, Instant}};
use tokio::sync::Mutex;
use tracing::{error, info, warn};
use zmq;
use mev::{
    communication::{GeyserUpdate, ZmqTopic, ZMQ_DATA_ENDPOINT, SimpleTransactionUpdate},
    decoders::{PoolFactory, PoolOperations},
    filtering::{cache::PoolCache, PoolIdentity},
};

const HOT_TRANSACTION_THRESHOLD: usize = 5;
const ACTIVITY_WINDOW_SECS: u64 = 120;
const HOTLIST_FILE_NAME: &str = "hotlist.json";
const UNIVERSE_FILE_NAME: &str = "pools_universe.json";

lazy_static::lazy_static! {
    static ref UNIVERSE_FILE_LOCK: Mutex<()> = Mutex::new(());
}

async fn run_scanner() -> Result<()> {
    info!("[Scanner] Démarrage du Market Scanner.");
    let rpc_url = std::env::var("SOLANA_RPC_URL")?;
    let rpc_client = std::sync::Arc::new(mev::rpc::ResilientRpcClient::new(rpc_url, 3, 500));
    let pool_factory = PoolFactory::new(rpc_client);
    let mut cache = PoolCache::load()?;
    let mut activity_tracker: HashMap<Pubkey, Vec<Instant>> = HashMap::new();
    let mut hotlist: HashSet<Pubkey> = HashSet::new();
    let mut last_hotlist_update = Instant::now();

    let context = zmq::Context::new();
    let subscriber = context.socket(zmq::SUB)?;
    subscriber.connect(ZMQ_DATA_ENDPOINT)?;
    subscriber.set_subscribe(&bincode::serialize(&ZmqTopic::Transaction)?)?;
    info!("[Scanner] Abonné au topic 'Transaction'. En attente...");

    loop {
        let multipart = subscriber.recv_multipart(0)?;
        if multipart.len() != 2 { continue; }

        if let Ok(GeyserUpdate::Transaction(tx_update)) = bincode::deserialize(&multipart[1]) {
            if let Some(identity) = discover_new_pool(&tx_update, &pool_factory) {
                if !cache.pools.contains_key(&identity.address) {
                    info!("[Scanner] ✨ Nouvelle pool détectée : {}", identity.address);
                    if let Err(e) = add_pool_to_universe(identity, &mut cache).await {
                        warn!("[Scanner] Erreur ajout au cache : {}", e);
                    }
                }
            }
            track_activity(&tx_update, &cache, &mut activity_tracker);
        }
        if last_hotlist_update.elapsed() > Duration::from_secs(10) {
            update_hotlist(&mut activity_tracker, &mut hotlist, &cache).await?;
            last_hotlist_update = Instant::now();
        }
    }
}

fn discover_new_pool(tx_update: &SimpleTransactionUpdate, pool_factory: &PoolFactory) -> Option<PoolIdentity> {
    for ix in &tx_update.instructions {
        if let Some(program_id) = tx_update.account_keys.get(ix.program_id_index as usize) {
            if let Some(first_account_idx) = ix.accounts.get(0) {
                if let Some(pool_addr) = tx_update.account_keys.get(*first_account_idx as usize) {
                    if let Ok(pool) = pool_factory.decode_raw_pool(pool_addr, &ix.data, program_id) {
                        let (mint_a, mint_b) = pool.get_mints();
                        let (vault_a, vault_b) = pool.get_vaults();
                        let accounts_to_watch = match pool {
                            mev::decoders::Pool::RaydiumClmm(_) | mev::decoders::Pool::OrcaWhirlpool(_) |
                            mev::decoders::Pool::MeteoraDlmm(_) | mev::decoders::Pool::MeteoraDammV2(_) => vec![pool.address()],
                            _ => vec![vault_a, vault_b],
                        };
                        return Some(PoolIdentity { address: pool.address(), mint_a, mint_b, accounts_to_watch });
                    }
                }
            }
        }
    }
    None
}



/// Ajoute un nouveau pool au fichier `pools_universe.json` et au cache en mémoire.
async fn add_pool_to_universe(new_identity: PoolIdentity, cache: &mut PoolCache) -> Result<()> {
    let _lock = UNIVERSE_FILE_LOCK.lock().await;

    // Mise à jour du cache en mémoire
    for watch_account in &new_identity.accounts_to_watch {
        cache.watch_map.insert(*watch_account, new_identity.address);
    }
    cache.pools.insert(new_identity.address, new_identity.clone());

    // Sauvegarde sur le disque
    let identities: Vec<PoolIdentity> = cache.pools.values().cloned().collect();
    let file = std::fs::File::create(UNIVERSE_FILE_NAME)?;
    let writer = std::io::BufWriter::new(file);
    serde_json::to_writer_pretty(writer, &identities)?;
    info!("[Scanner] ✅ Le pool {} a été ajouté à pools_universe.json !", new_identity.address);
    Ok(())
}

/// Met à jour le tracker d'activité basé sur les comptes impliqués dans la transaction.
fn track_activity(
    tx_update: &SimpleTransactionUpdate,
    cache: &PoolCache,
    activity_tracker: &mut HashMap<Pubkey, Vec<Instant>>,
) {
    let mut found_pools = HashSet::new();
    for key in &tx_update.account_keys {
        if let Some(pool_address) = cache.watch_map.get(key) {
            if !found_pools.contains(pool_address) {
                activity_tracker.entry(*pool_address).or_default().push(Instant::now());
                found_pools.insert(*pool_address);
            }
        }
    }
}

/// Analyse le tracker d'activité et met à jour le fichier `hotlist.json`.
async fn update_hotlist(
    activity_tracker: &mut HashMap<Pubkey, Vec<Instant>>,
    current_hotlist: &mut HashSet<Pubkey>,
    cache: &PoolCache,
) -> Result<()> {
    let mut new_hotlist = HashSet::new();
    let now = Instant::now();

    activity_tracker.retain(|pool_address, timestamps| {
        timestamps.retain(|ts| now.duration_since(*ts).as_secs() < ACTIVITY_WINDOW_SECS);
        if timestamps.len() >= HOT_TRANSACTION_THRESHOLD {
            if let Some(_identity) = cache.pools.get(pool_address) {
                // On pourrait ajouter d'autres filtres ici (ex: whitelist de tokens)
                new_hotlist.insert(*pool_address);
            }
        }
        !timestamps.is_empty()
    });

    if *current_hotlist != new_hotlist {
        info!("[Scanner] Hotlist mise à jour. {} pools actifs.", new_hotlist.len());
        fs::write(HOTLIST_FILE_NAME, serde_json::to_string_pretty(&new_hotlist)?)?;
        *current_hotlist = new_hotlist;
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    mev::monitoring::logging::setup_logging();
    dotenvy::dotenv().ok();
    info!("[Scanner] Démarrage du service Market Scanner.");
    if let Err(e) = run_scanner().await {
        error!("[Scanner] Le service a planté : {:?}.", e);
    }
    Ok(())
}