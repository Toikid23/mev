
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
use mev::{
    config::Config,
};
use std::str::FromStr;


lazy_static::lazy_static! {
    // Whitelist de tokens de confiance. On ne suit que les pools qui contiennent au moins un de ces tokens.
    // Pour l'instant, uniquement le Wrapped SOL.
    static ref TOKEN_WHITELIST: HashSet<Pubkey> = {
        let mut set = HashSet::new();
        // Wrapped SOL (WSOL)
        set.insert(Pubkey::from_str("So11111111111111111111111111111111111111112").unwrap());
        set
    };
}

const HOTLIST_FILE_NAME: &str = "hotlist.json";
const UNIVERSE_FILE_NAME: &str = "pools_universe.json";

lazy_static::lazy_static! {
    static ref UNIVERSE_FILE_LOCK: Mutex<()> = Mutex::new(());
}

async fn run_scanner(config: Config) -> Result<()> {
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
            // --- PASSEZ LES VALEURS DE LA CONFIG ICI ---
            update_hotlist(
                &mut activity_tracker,
                &mut hotlist,
                &cache,
                config.hot_transaction_threshold,
                config.activity_window_secs,
            ).await?;
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

async fn update_hotlist(
    activity_tracker: &mut HashMap<Pubkey, Vec<Instant>>,
    current_hotlist: &mut HashSet<Pubkey>,
    cache: &PoolCache,
    hot_threshold: usize,
    activity_window: u64,
) -> Result<()> {
    let mut new_hotlist = HashSet::new();
    let now = Instant::now();

    activity_tracker.retain(|pool_address, timestamps| {
        // Étape 1 : Nettoyer les anciennes transactions de la fenêtre
        timestamps.retain(|ts| now.duration_since(*ts).as_secs() < activity_window);

        // Étape 2 : Vérifier si le pool est "chaud"
        if timestamps.len() >= hot_threshold {
            // --- FILTRE DE WHITELIST AJOUTÉ ICI ---
            if let Some(identity) = cache.pools.get(pool_address) {
                // Étape 3 : Vérifier si le pool contient un token de notre whitelist (WSOL)
                if TOKEN_WHITELIST.contains(&identity.mint_a) || TOKEN_WHITELIST.contains(&identity.mint_b) {
                    new_hotlist.insert(*pool_address);
                }
            }
            // --- FIN DU FILTRE ---
        }

        // On garde le pool dans le tracker tant qu'il a une activité récente
        !timestamps.is_empty()
    });

    if *current_hotlist != new_hotlist {
        info!("[Scanner] Hotlist mise à jour. {} pools actifs contenant du WSOL.", new_hotlist.len());
        fs::write(HOTLIST_FILE_NAME, serde_json::to_string_pretty(&new_hotlist)?)?;
        *current_hotlist = new_hotlist;
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    mev::monitoring::logging::setup_logging();
    dotenvy::dotenv().ok();

    // --- CHARGEZ LA CONFIG ICI ---
    let config = Config::load()?;

    info!("[Scanner] Démarrage du service Market Scanner.");
    // --- PASSEZ LA CONFIG À `run_scanner` ---
    if let Err(e) = run_scanner(config).await {
        error!("[Scanner] Le service a planté : {:?}.", e);
    }
    Ok(())
}
