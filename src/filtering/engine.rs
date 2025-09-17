// DANS : src/filtering/engine.rs

use crate::{
    decoders::{PoolOperations},
    filtering::cache::PoolCache,
};
use crate::graph_engine::Graph;
use anyhow::{Result};
use solana_sdk::pubkey::Pubkey;
use std::{
    collections::{HashMap, HashSet},
    fs,
    time::{Duration, Instant},
};
use tokio::sync::mpsc;
use crate::rpc::ResilientRpcClient;
use std::str::FromStr;
use std::sync::Arc;
use crate::decoders::PoolFactory;

// --- CONSTANTES DE CONFIGURATION DU FILTRAGE ---
// Un pool est "chaud" s'il a eu au moins 5 transactions...
const HOT_TRANSACTION_THRESHOLD: usize = 5;
// ...au cours des 2 dernières minutes (120 secondes).
const ACTIVITY_WINDOW_SECS: u64 = 120;
// Fichier de sortie pour la hotlist.
const HOTLIST_FILE_NAME: &str = "hotlist.json";

lazy_static::lazy_static! {
    // Whitelist de tokens de confiance. On ne suit que les pools qui contiennent au moins un de ces tokens.
    static ref TOKEN_WHITELIST: HashSet<Pubkey> = {
        let mut set = HashSet::new();
        set.insert(Pubkey::from_str("So11111111111111111111111111111111111111112").unwrap()); // SOL
        set.insert(Pubkey::from_str("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v").unwrap()); // USDC
        // Ajoutez d'autres tokens de confiance ici si nécessaire
        set
    };
}

pub struct FilteringEngine {
    rpc_client: Arc<ResilientRpcClient>,
    pool_factory: PoolFactory, // <-- AJOUTEZ CE CHAMP
}

impl FilteringEngine {
    pub fn new(rpc_url: String) -> Self {
        let rpc_client = Arc::new(ResilientRpcClient::new(rpc_url, 3, 500));
        let pool_factory = PoolFactory::new(rpc_client.clone()); // <-- INSTANCIEZ LA FACTORY
        Self {
            rpc_client,
            pool_factory, // <-- STOCKEZ-LA
        }
    }

    pub async fn run(
        &self,
        mut active_pool_receiver: mpsc::Receiver<Pubkey>,
        initial_cache: PoolCache,
    ) {
        println!("[Engine] Moteur d'analyse démarré avec des seuils de filtrage stricts.");
        let mut activity_tracker: HashMap<Pubkey, Vec<Instant>> = HashMap::new();
        let mut known_pools: HashMap<Pubkey, super::PoolIdentity> = HashMap::new();
        let mut hotlist: HashSet<Pubkey> = HashSet::new();
        let mut analysis_interval = tokio::time::interval(Duration::from_secs(10));

        let mut reload_cache_interval = tokio::time::interval(Duration::from_secs(30)); // Recharger toutes les 30s
        let mut cache = initial_cache; // On rend le cache mutable

        // <-- MODIFIÉ : Pré-allocation des collections utilisées dans la boucle d'analyse
        let mut hot_pairs: HashSet<(Pubkey, Pubkey)> = HashSet::with_capacity(100);
        let mut new_hotlist: HashSet<Pubkey> = HashSet::with_capacity(500);


        loop {
            tokio::select! {
        // La branche active_pool_receiver reste inchangée
        Some(pool_address) = active_pool_receiver.recv() => {
            if let std::collections::hash_map::Entry::Vacant(entry) = known_pools.entry(pool_address) {
                // On utilise maintenant le cache mutable
                if let Some(identity) = cache.pools.get(&pool_address) {
                    // ... le reste de la logique est inchangé
                    if !TOKEN_WHITELIST.contains(&identity.mint_a) && !TOKEN_WHITELIST.contains(&identity.mint_b) {
                        continue;
                    }
                    match self.hydrate_and_check_liquidity(identity).await {
                        Ok(true) => {
                            println!("[Engine] Nouveau pool {} a passé le filtre statique. Suivi de l'activité.", pool_address);
                            entry.insert(identity.clone());
                            activity_tracker.entry(pool_address).or_default().push(Instant::now());
                        },
                        Ok(false) => { /* Ignore */ },
                        Err(e) => eprintln!("[Engine] Erreur d'hydratation pour {}: {}", pool_address, e),
                    }
                }
            } else {
                activity_tracker.entry(pool_address).or_default().push(Instant::now());
            }
        },

        // --- NOUVELLE BRANCHE À AJOUTER ---
        _ = reload_cache_interval.tick() => {
            println!("[Engine] Tentative de rechargement du cache de l'univers des pools...");
            match PoolCache::load() {
                Ok(new_cache) => {
                    if new_cache.pools.len() > cache.pools.len() {
                        println!("[Engine] ✅ Cache rechargé avec succès. {} nouveaux pools détectés (total: {}).",
                                 new_cache.pools.len() - cache.pools.len(),
                                 new_cache.pools.len());
                        cache = new_cache; // On remplace l'ancien cache par le nouveau
                    }
                },
                Err(e) => {
                    eprintln!("[Engine] ⚠️ Erreur lors du rechargement du cache : {}", e);
                }
            }
        },
                _ = analysis_interval.tick() => {
                    // <-- MODIFIÉ : Vider les collections au lieu de les recréer
                    hot_pairs.clear();
                    new_hotlist.clear();

                    let now = Instant::now();
                    activity_tracker.retain(|pool_address, timestamps| {
                        timestamps.retain(|ts| now.duration_since(*ts).as_secs() < ACTIVITY_WINDOW_SECS);
                        if timestamps.len() >= HOT_TRANSACTION_THRESHOLD {
                            if let Some(identity) = known_pools.get(pool_address) {
                                let (mut mint_a, mut mint_b) = (identity.mint_a, identity.mint_b);
                                if mint_a > mint_b { std::mem::swap(&mut mint_a, &mut mint_b); }
                                hot_pairs.insert((mint_a, mint_b));
                            }
                        }
                        !timestamps.is_empty()
                    });

                    for (mint_a, mint_b) in &hot_pairs {
                for identity in cache.pools.values() { // <-- Utilise `cache` au lieu de `initial_cache`
                    let (mut id_mint_a, mut id_mint_b) = (identity.mint_a, identity.mint_b);
                    if id_mint_a > id_mint_b { std::mem::swap(&mut id_mint_a, &mut id_mint_b); }
                    if id_mint_a == *mint_a && id_mint_b == *mint_b {
                        new_hotlist.insert(identity.address);
                    }
                }
            }

                    if !new_hotlist.is_empty() {
                        println!("[MEM_METRICS] engine.rs - new_hotlist length: {}", new_hotlist.len());
                    }
                    if !hot_pairs.is_empty() {
                        println!("[MEM_METRICS] engine.rs - hot_pairs length: {}", hot_pairs.len());
                    }

                    if hotlist != new_hotlist {
                        println!("[Engine] Hotlist mise à jour. {} pools au total.", new_hotlist.len());
                        if let Err(e) = fs::write(HOTLIST_FILE_NAME, serde_json::to_string_pretty(&new_hotlist).unwrap()) {
                            eprintln!("[Engine] Erreur d'écriture de la hotlist: {}", e);
                        }
                        hotlist = new_hotlist.clone(); // <-- MODIFIÉ : Cloner la nouvelle hotlist pour la sauvegarder
                    }
                }
            }
        }
    }

    /// Décode, hydrate et vérifie si le pool a une liquidité minimale.
    async fn hydrate_and_check_liquidity(&self, identity: &super::PoolIdentity) -> Result<bool> {
        let account = self.rpc_client.get_account(&identity.address).await?;
        let raw_pool = self.pool_factory.decode_raw_pool(&identity.address, &account.data, &account.owner)?;
        let hydrated_pool = Graph::hydrate_pool(raw_pool, &self.rpc_client).await?;

        // Approximation de la liquidité : on vérifie juste que les réserves ne sont pas nulles.
        // C'est un filtre simple mais efficace contre les pools vides ou défectueux.
        let (reserve_a, reserve_b) = hydrated_pool.get_reserves();
        Ok(reserve_a > 0 && reserve_b > 0)
    }

}