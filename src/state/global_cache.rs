// DANS : src/state/global_cache.rs

use crate::decoders::{
    pump::amm::pool::onchain_layouts as pump_layouts,
    raydium::cpmm::config::DecodedAmmConfig as CpmmDecodedConfig,
    raydium::clmm::config::DecodedClmmConfig as ClmmDecodedConfig,
    orca::whirlpool::config::DecodedWhirlpoolsConfig,
};
use solana_sdk::pubkey::Pubkey;
use std::{
    collections::HashMap,
    sync::RwLock,
    time::{Duration, Instant},
};
use lazy_static::lazy_static;

// Durée de vie d' une entrée dans le cache avant qu'elle soit considérée comme expirée.
const CACHE_TTL_SECONDS: u64 = 300; // 5 minutes

// Une entrée générique pour notre cache.
// Elle contient la donnée (T) et le moment où elle a été insérée.
#[derive(Debug, Clone)]
struct CacheEntry<T> {
    data: T,
    inserted_at: Instant,
}

impl<T> CacheEntry<T> {
    // Vérifie si l'entrée est toujours valide.
    fn is_valid(&self) -> bool {
        self.inserted_at.elapsed() < Duration::from_secs(CACHE_TTL_SECONDS)
    }
}

// L'énumération de toutes les données que nous pouvons mettre en cache.
// C'est plus propre et plus sûr que d'utiliser des types génériques partout.
#[derive(Debug, Clone)]
pub enum CacheableData {
    PumpAmmGlobalConfig(pump_layouts::GlobalConfig),
    RaydiumCpmmAmmConfig(CpmmDecodedConfig),
    RaydiumClmmAmmConfig(ClmmDecodedConfig),
    OrcaWhirlpoolsConfig(DecodedWhirlpoolsConfig),
    // Nous pourrons ajouter d'autres types de config ici à l'avenir.
}

// La structure principale de notre cache.
// Elle est conçue pour être utilisée dans un contexte multi-thread.
pub struct GlobalCache {
    // La clé est l'adresse du compte de configuration.
    cache: RwLock<HashMap<Pubkey, CacheEntry<CacheableData>>>,
}

impl GlobalCache {
    // Crée une nouvelle instance vide du cache.
    pub fn new() -> Self {
        Self {
            cache: RwLock::new(HashMap::new()),
        }
    }

    // Tente de récupérer une donnée depuis le cache.
    pub fn get(&self, key: &Pubkey) -> Option<CacheableData> {
        let reader = self.cache.read().unwrap();
        reader.get(key).and_then(|entry| {
            if entry.is_valid() {
                Some(entry.data.clone()) // On retourne une copie de la donnée.
            } else {
                None // L'entrée a expiré.
            }
        })
    }

    // Insère ou met à jour une donnée dans le cache.
    pub fn put(&self, key: Pubkey, data: CacheableData) {
        let mut writer = self.cache.write().unwrap();
        let entry = CacheEntry {
            data,
            inserted_at: Instant::now(),
        };
        writer.insert(key, entry);
    }
}

// Déclaration de notre instance de cache globale et unique.
// Elle sera initialisée de manière "paresseuse" (lazy) et thread-safe.
lazy_static! {
    pub static ref GLOBAL_CACHE: GlobalCache = GlobalCache::new();
}