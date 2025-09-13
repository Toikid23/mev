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
        let mut activity_tracker: HashMap<Pubkey, Vec<Instant>> = HashMap::new();
        let mut known_pools: HashMap<Pubkey, super::PoolIdentity> = HashMap::new();
        let mut hotlist: HashSet<Pubkey> = HashSet::new();
        let mut analysis_interval = tokio::time::interval(Duration::from_secs(10));

        loop {
            tokio::select! {
                Some(pool_address) = active_pool_receiver.recv() => {
                    if !known_pools.contains_key(&pool_address) {
                        if let Some(identity) = initial_cache.pools.get(&pool_address) {
                            if !TOKEN_WHITELIST.contains(&identity.mint_a) && !TOKEN_WHITELIST.contains(&identity.mint_b) {
                                continue; // Le pool ne contient aucun token de confiance, on l'ignore.
                            }
                            match self.hydrate_and_check_liquidity(identity).await {
                                Ok(true) => {
                                    println!("[Engine] Nouveau pool {} a passé le filtre statique. Suivi de l'activité.", pool_address);
                                    known_pools.insert(pool_address, identity.clone());
                                    activity_tracker.entry(pool_address).or_default().push(Instant::now());
                                },
                                Ok(false) => { /* Le pool n'a pas assez de liquidité, on l'ignore. */ },
                                Err(e) => eprintln!("[Engine] Erreur d'hydratation pour {}: {}", pool_address, e),
                            }
                        }
                    } else {
                        activity_tracker.entry(pool_address).or_default().push(Instant::now());
                    }
                },
                _ = analysis_interval.tick() => {
                    let mut hot_pairs: HashSet<(Pubkey, Pubkey)> = HashSet::new();
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

                    let mut new_hotlist = HashSet::new();
                    for (mint_a, mint_b) in hot_pairs {
                        for identity in initial_cache.pools.values() {
                            let (mut id_mint_a, mut id_mint_b) = (identity.mint_a, identity.mint_b);
                            if id_mint_a > id_mint_b { std::mem::swap(&mut id_mint_a, &mut id_mint_b); }
                            if id_mint_a == mint_a && id_mint_b == mint_b {
                                new_hotlist.insert(identity.address);
                            }
                        }
                    }

                    if hotlist != new_hotlist {
                        println!("[Engine] Hotlist mise à jour. {} pools au total.", new_hotlist.len());
                        if let Err(e) = fs::write(HOTLIST_FILE_NAME, serde_json::to_string_pretty(&new_hotlist).unwrap()) {
                            eprintln!("[Engine] Erreur d'écriture de la hotlist: {}", e);
                        }
                        hotlist = new_hotlist;
                    }
                }
            }
        }
    }

    /// Décode, hydrate et vérifie si le pool a une liquidité minimale.
    async fn hydrate_and_check_liquidity(&self, identity: &super::PoolIdentity) -> Result<bool> {
        let account = self.rpc_client.get_account(&identity.address).await?;
        let raw_pool = self.decode_raw_pool(&identity.address, &account.data, &account.owner)?;
        let graph = crate::graph_engine::Graph::new();
        let hydrated_pool = graph.hydrate_pool(raw_pool, &self.rpc_client).await?;

        // Approximation de la liquidité : on vérifie juste que les réserves ne sont pas nulles.
        // C'est un filtre simple mais efficace contre les pools vides ou défectueux.
        let (reserve_a, reserve_b) = hydrated_pool.get_reserves();
        Ok(reserve_a > 0 && reserve_b > 0)
    }

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