// DANS : src/state/global_cache.rs

use crate::decoders::{
    orca::whirlpool::config::DecodedWhirlpoolsConfig,
    pump::amm::pool::onchain_layouts as pump_layouts,
    raydium::clmm::config::DecodedClmmConfig,
    raydium::cpmm::config::DecodedAmmConfig as CpmmDecodedConfig,
};
use crate::rpc::ResilientRpcClient;
use anyhow::{Context, Result};
use lazy_static::lazy_static;
use solana_sdk::{
    pubkey::Pubkey,
    sysvar::clock::{self, Clock},
};
use std::{
    collections::HashMap,
    sync::RwLock,
    time::{Duration, Instant},
};

const CONFIG_CACHE_TTL_SECONDS: u64 = 300;
const CLOCK_CACHE_TTL_MILLIS: u64 = 500;

#[derive(Debug, Clone)]
struct CacheEntry<T> {
    data: T,
    inserted_at: Instant,
}

impl<T> CacheEntry<T> {
    fn is_valid(&self, custom_ttl: Duration) -> bool {
        self.inserted_at.elapsed() < custom_ttl
    }
}

#[derive(Debug, Clone)]
pub enum CacheableData {
    // --- CORRECTION : On boxe la variante la plus grande ---
    PumpAmmGlobalConfig(Box<pump_layouts::GlobalConfig>),

    RaydiumCpmmAmmConfig(CpmmDecodedConfig),
    RaydiumClmmAmmConfig(DecodedClmmConfig),
    OrcaWhirlpoolsConfig(DecodedWhirlpoolsConfig),
    SysvarClock(Clock),
}

pub struct GlobalCache {
    cache: RwLock<HashMap<Pubkey, CacheEntry<CacheableData>>>,
}

// --- CORRECTION : On implémente le trait Default comme suggéré par Clippy ---
impl Default for GlobalCache {
    fn default() -> Self {
        Self::new()
    }
}

impl GlobalCache {
    pub fn new() -> Self {
        Self {
            cache: RwLock::new(HashMap::new()),
        }
    }

    pub fn get(&self, key: &Pubkey) -> Option<CacheableData> {
        let reader = self.cache.read().unwrap();
        reader.get(key).and_then(|entry| {
            let ttl = match entry.data {
                CacheableData::SysvarClock(_) => Duration::from_millis(CLOCK_CACHE_TTL_MILLIS),
                _ => Duration::from_secs(CONFIG_CACHE_TTL_SECONDS),
            };
            if entry.is_valid(ttl) {
                Some(entry.data.clone())
            } else {
                None
            }
        })
    }

    pub fn put(&self, key: Pubkey, data: CacheableData) {
        let mut writer = self.cache.write().unwrap();
        let entry = CacheEntry {
            data,
            inserted_at: Instant::now(),
        };
        writer.insert(key, entry);
    }
}

lazy_static! {
    pub static ref GLOBAL_CACHE: GlobalCache = GlobalCache::new();
}

/// Récupère le sysvar Clock, en utilisant le cache global.
/// Fait un appel RPC uniquement si le cache est vide ou a expiré.
pub async fn get_cached_clock(rpc_client: &ResilientRpcClient) -> Result<Clock> {
    let clock_address = clock::ID;

    if let Some(CacheableData::SysvarClock(clock)) = GLOBAL_CACHE.get(&clock_address) {
        println!("[Cache] HIT pour SysvarClock.");
        return Ok(clock);
    }

    println!("[Cache] MISS pour SysvarClock. Fetching via RPC...");
    let clock_account = rpc_client
        .get_account(&clock_address)
        .await
        .context("Échec de la récupération du compte Sysvar Clock")?;

    let clock: Clock = bincode::deserialize(&clock_account.data)?;

    GLOBAL_CACHE.put(clock_address, CacheableData::SysvarClock(clock.clone()));

    Ok(clock)
}