// DANS : src/filtering/engine.rs

use crate::{
    decoders::{Pool, PoolOperations},
    filtering::cache::PoolCache,
    graph_engine::Graph,
    decoders::{raydium, orca, meteora, pump},
};
use anyhow::{anyhow, Result};
use solana_sdk::pubkey::Pubkey;
use std::{
    collections::{HashMap, HashSet},
    fs,
    sync::Arc,
    time::{Duration, Instant},
};
// On va utiliser le RwLock synchrone de std pour la hotlist, car les accès sont simples.
use std::sync::RwLock;
use tokio::sync::mpsc;
use crate::rpc::ResilientRpcClient;
use std::str::FromStr;


const HOTLIST_FILE_NAME: &str = "hotlist.json";
const HOTLIST_SIZE: usize = 20;
const ACTIVITY_WINDOW_SECS: u64 = 300;

#[derive(Clone)]
pub struct FilteringEngine {
    rpc_client: Arc<ResilientRpcClient>,
    // CORRIGÉ : Le graph_engine doit être un pointeur Arc partagé,
    // car la structure Graph elle-même n'est plus clonable.
    graph_engine: Arc<Graph>,
    hotlist: Arc<RwLock<HashMap<Pubkey, Instant>>>,
}

impl FilteringEngine {
    pub fn new(rpc_url: String) -> Self {
        Self {
            rpc_client: Arc::new(ResilientRpcClient::new(rpc_url, 3, 500)),
            // CORRIGÉ : On initialise le Graph à l'intérieur de l'Arc.
            graph_engine: Arc::new(Graph::new()),
            hotlist: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn run(&self, mut active_pool_receiver: mpsc::Receiver<Pubkey>, initial_cache: PoolCache) {
        println!("[Engine] Moteur d'analyse démarré.");

        let hotlist_clone = self.hotlist.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(5));
            loop {
                interval.tick().await;
                if let Err(e) = Self::update_hotlist_file(&hotlist_clone) {
                    eprintln!("[Engine] Erreur lors de la mise à jour du fichier hotlist: {:?}", e);
                }
            }
        });

        while let Some(pool_address) = active_pool_receiver.recv().await {
            if let Some(identity) = initial_cache.pools.get(&pool_address) {
                let should_process = {
                    let reader = self.hotlist.read().unwrap();
                    match reader.get(&pool_address) {
                        Some(last_processed) => last_processed.elapsed() > Duration::from_secs(10),
                        None => true,
                    }
                };

                if should_process {
                    let engine_clone = self.clone();
                    let identity_clone = identity.clone();
                    tokio::spawn(async move {
                        if let Err(e) = engine_clone.process_active_pool(identity_clone).await {
                            eprintln!("[Engine] Erreur lors du traitement du pool {}: {:?}", pool_address, e);
                        }
                    });
                }
            }
        }
    }

    async fn process_active_pool(&self, identity: super::PoolIdentity) -> Result<()> {
        let account = self.rpc_client.get_account(&identity.address).await?;
        let raw_pool = self.decode_raw_pool(&identity.address, &account.data, &account.owner)?;

        // L'appel à hydrate_pool est correct, il est appelé sur l'instance de Graph.
        let hydrated_pool = self.graph_engine.hydrate_pool(raw_pool, &self.rpc_client).await?;

        if self.passes_final_filters(&hydrated_pool) {
            let mut writer = self.hotlist.write().unwrap();
            writer.insert(identity.address, Instant::now());
            println!("[Engine] ✅ Pool {} ajouté/mis à jour dans la hotlist.", identity.address);
        }

        Ok(())
    }

    // Le reste de la fonction ne change pas, car il est déjà correct.
    fn update_hotlist_file(hotlist_arc: &Arc<RwLock<HashMap<Pubkey, Instant>>>) -> Result<()> {
        let reader = hotlist_arc.read().unwrap();
        let mut active_pools: Vec<(Pubkey, Instant)> = reader.iter().filter(|(_, instant)| instant.elapsed().as_secs() < ACTIVITY_WINDOW_SECS).map(|(k, v)| (*k, *v)).collect();
        active_pools.sort_by(|a, b| b.1.cmp(&a.1));
        let final_hotlist: HashSet<Pubkey> = active_pools.into_iter().take(HOTLIST_SIZE).map(|(k, _)| k).collect();
        let json_data = serde_json::to_string_pretty(&final_hotlist)?;
        fs::write(HOTLIST_FILE_NAME, json_data)?;
        Ok(())
    }

    fn passes_final_filters(&self, pool: &Pool) -> bool {
        let (reserve_a, reserve_b) = pool.get_reserves();
        if reserve_a == 0 && reserve_b == 0 {
            return false;
        }
        true
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