// DANS : src/filtering/engine.rs

use crate::{
    decoders::{Pool, PoolOperations},
    filtering::cache::PoolCache,
    decoders::{raydium, orca, meteora, pump},
};
use anyhow::{anyhow, Result};
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

// --- CONSTANTES DE CONFIGURATION DU FILTRAGE ---
// Un pool doit avoir au moins 500$ de liquidité pour être considéré.
const MINIMUM_LIQUIDITY_USD: f64 = 500.0;
// Un pool est "chaud" s'il a eu au moins 5 transactions...
const HOT_TRANSACTION_THRESHOLD: usize = 5;
// ...au cours des 2 dernières minutes (120 secondes).
const ACTIVITY_WINDOW_SECS: u64 = 120;
// Fichier de sortie pour la hotlist.
const HOTLIST_FILE_NAME: &str = "hotlist.json";


pub struct FilteringEngine {
    rpc_client: Arc<ResilientRpcClient>,
}

impl FilteringEngine {
    pub fn new(rpc_url: String) -> Self {
        Self {
            rpc_client: Arc::new(ResilientRpcClient::new(rpc_url, 3, 500)),
        }
    }

    pub async fn run(
        &self,
        mut active_pool_receiver: mpsc::Receiver<Pubkey>,
        initial_cache: PoolCache,
    ) {
        println!("[Engine] Moteur d'analyse démarré avec des seuils de filtrage stricts.");

        // `activity_tracker` stocke les timestamps des transactions récentes pour chaque pool.
        let mut activity_tracker: HashMap<Pubkey, Vec<Instant>> = HashMap::new();
        // `known_pools` stocke les `PoolIdentity` des pools qui ont passé le filtre statique.
        let mut known_pools: HashMap<Pubkey, super::PoolIdentity> = HashMap::new();
        // `hotlist` est l'ensemble final des pools qui seront écrits sur le disque.
        let mut hotlist: HashSet<Pubkey> = HashSet::new();

        // On déclenche l'analyse et la mise à jour de la hotlist toutes les 10 secondes.
        let mut analysis_interval = tokio::time::interval(Duration::from_secs(10));

        loop {
            tokio::select! {
                // Branche 1: Gérer un nouveau pool actif signalé par le Watcher.
                Some(pool_address) = active_pool_receiver.recv() => {
                    // Si nous n'avons jamais vu ce pool, nous devons faire le filtrage statique.
                    if !known_pools.contains_key(&pool_address) {
                        if let Some(identity) = initial_cache.pools.get(&pool_address) {
                            // On décode et hydrate le pool pour vérifier sa liquidité.
                            match self.hydrate_and_filter_pool(identity).await {
                                Ok(Some(_)) => {
                                    // Le pool a assez de liquidité, on l'ajoute à nos listes.
                                    println!("[Engine] Nouveau pool {} a passé le filtre statique. Suivi de l'activité.", pool_address);
                                    known_pools.insert(pool_address, identity.clone());
                                    activity_tracker.entry(pool_address).or_default().push(Instant::now());
                                },
                                Ok(None) => {
                                    // Le pool n'a pas assez de liquidité, on l'ignore.
                                },
                                Err(e) => {
                                    eprintln!("[Engine] Erreur d'hydratation pour {}: {}", pool_address, e);
                                }
                            }
                        }
                    } else {
                        // Si on connaît déjà le pool, on enregistre simplement la nouvelle activité.
                        activity_tracker.entry(pool_address).or_default().push(Instant::now());
                    }
                },

                // Branche 2: Analyser périodiquement l'activité et mettre à jour la hotlist.
                _ = analysis_interval.tick() => {
                    let mut hot_pairs: HashSet<(Pubkey, Pubkey)> = HashSet::new();
                    let now = Instant::now();

                    // Nettoyer et analyser l'activité
                    activity_tracker.retain(|pool_address, timestamps| {
                        // 1. On supprime les timestamps trop anciens.
                        timestamps.retain(|ts| now.duration_since(*ts).as_secs() < ACTIVITY_WINDOW_SECS);

                        // 2. Si le pool est suffisamment actif...
                        if timestamps.len() >= HOT_TRANSACTION_THRESHOLD {
                            // ...on marque sa paire de tokens comme "chaude".
                            if let Some(identity) = known_pools.get(pool_address) {
                                let (mut mint_a, mut mint_b) = (identity.mint_a, identity.mint_b);
                                if mint_a > mint_b { std::mem::swap(&mut mint_a, &mut mint_b); }
                                hot_pairs.insert((mint_a, mint_b));
                            }
                        }
                        // On garde le pool dans le tracker tant qu'il a une activité récente.
                        !timestamps.is_empty()
                    });

                    // Construire la nouvelle hotlist
                    let mut new_hotlist = HashSet::new();
                    for (mint_a, mint_b) in hot_pairs {
                        // Pour chaque paire chaude, on trouve TOUS les pools correspondants dans notre cache initial.
                        for identity in initial_cache.pools.values() {
                            let (mut id_mint_a, mut id_mint_b) = (identity.mint_a, identity.mint_b);
                            if id_mint_a > id_mint_b { std::mem::swap(&mut id_mint_a, &mut id_mint_b); }

                            if id_mint_a == mint_a && id_mint_b == mint_b {
                                new_hotlist.insert(identity.address);
                            }
                        }
                    }

                    // Mettre à jour et écrire sur le disque seulement s'il y a un changement.
                    if hotlist != new_hotlist {
                        println!("[Engine] Hotlist mise à jour. {} paires actives, {} pools au total.", new_hotlist.len() / 2, new_hotlist.len()); // Approximation
                        if let Err(e) = fs::write(HOTLIST_FILE_NAME, serde_json::to_string_pretty(&new_hotlist).unwrap()) {
                            eprintln!("[Engine] Erreur d'écriture de la hotlist: {}", e);
                        }
                        hotlist = new_hotlist;
                    }
                }
            }
        }
    }

    /// Décode, hydrate et filtre un pool en fonction de sa liquidité.
    async fn hydrate_and_filter_pool(&self, identity: &super::PoolIdentity) -> Result<Option<Pool>> {
        let account = self.rpc_client.get_account(&identity.address).await?;
        let raw_pool = self.decode_raw_pool(&identity.address, &account.data, &account.owner)?;

        // On utilise la fonction d'hydratation générique du Graph (même si on ne stocke pas le graphe ici).
        let graph = crate::graph_engine::Graph::new();
        let hydrated_pool = graph.hydrate_pool(raw_pool, &self.rpc_client).await?;

        // Pour l'instant, nous n'avons pas de moyen fiable d'obtenir le prix des tokens.
        // On va se contenter de filtrer les pools qui ont au moins des réserves.
        // TODO: Intégrer un oracle de prix (ex: Birdeye) pour un filtre de TVL précis.
        let (reserve_a, reserve_b) = hydrated_pool.get_reserves();
        if reserve_a > 0 && reserve_b > 0 {
            Ok(Some(hydrated_pool))
        } else {
            Ok(None)
        }
    }

    // La fonction de décodage reste la même que celle que vous aviez.
    fn decode_raw_pool(&self, address: &Pubkey, data: &[u8], owner: &Pubkey) -> Result<Pool> {
        match *owner {
            id if id == raydium::amm_v4::RAYDIUM_AMM_V4_PROGRAM_ID => raydium::amm_v4::decode_pool(address, data).map(Pool::RaydiumAmmV4),
            id if id == Pubkey::from_str("CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C").unwrap() => raydium::cpmm::decode_pool(address, data).map(Pool::RaydiumCpmm),
            id if id == Pubkey::from_str("CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK").unwrap() => raydium::clmm::decode_pool(address, data, &id).map(Pool::RaydiumClmm),
            id if id == Pubkey::from_str("Eo7WjKq67rjJQSZxS6z3YkapzY3eMj6Xy8X5EQVn5UaB").unwrap() => meteora::damm_v1::decode_pool(address, data).map(Pool::MeteoraDammV1),
            id if id == Pubkey::from_str("cpamdpZCGKUy5JxQXB4dcpGPiikHawvSWAd6mEn1sGG").unwrap() => meteora::damm_v2::decode_pool(address, data).map(Pool::MeteoraDammV2),
            id if id == Pubkey::from_str("LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo").unwrap() => meteora::dlmm::decode_lb_pair(address, data, &id).map(Pool::MeteoraDlmm),
            id if id == Pubkey::from_str("whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc").unwrap() => orca::whirlpool::decode_pool(address, data).map(Pool::OrcaWhirlpool),
            id if id == pump::amm::PUMP_PROGRAM_ID => pump::amm::decode_pool(address, data).map(Pool::PumpAmm),
            _ => Err(anyhow!("Programme propriétaire inconnu pour le décodage : {}", owner)),
        }
    }
}