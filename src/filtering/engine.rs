// DANS : src/filtering/engine.rs

use crate::{
    decoders::{Pool, PoolOperations},
    filtering::cache::PoolCache,
    graph_engine::Graph,
};
use anyhow::{Context, Result};
use solana_client::nonblocking::rpc_client::RpcClient; // On passe au client non-bloquant
use solana_sdk::pubkey::Pubkey;
use std::{
    collections::{HashMap, HashSet},
    fs,
    sync::{Arc, RwLock},
    time::{Duration, Instant},
};
use tokio::sync::mpsc;

const HOTLIST_FILE_NAME: &str = "hotlist.json";
const HOTLIST_SIZE: usize = 20;
const ACTIVITY_WINDOW_SECS: u64 = 300;

#[derive(Clone)] // Clone est plus simple à gérer ici
pub struct FilteringEngine {
    rpc_client: Arc<RpcClient>, // Utiliser un Arc pour le partage asynchrone
    graph_engine: Graph,
    hotlist: Arc<RwLock<HashMap<Pubkey, Instant>>>,
}

impl FilteringEngine {
    pub fn new(rpc_url: String) -> Self {
        Self {
            rpc_client: Arc::new(RpcClient::new(rpc_url)),
            graph_engine: Graph::new(),
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
                    println!("[Engine] Analyse du pool actif: {}", pool_address);
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
        let account_data = self.rpc_client.get_account_data(&identity.address).await?;
        let pool_owner = self.rpc_client.get_account(&identity.address).await?.owner;

        let raw_pool = self.decode_raw_pool(&identity.address, &account_data, &pool_owner)?;

        // --- CORRECTION 4: Remplacer todo!() par le vrai client RPC ---
        let hydrated_pool = self.graph_engine.hydrate_pool(raw_pool, &self.rpc_client).await?;

        // --- CORRECTION 5: Utiliser la variable pour enlever le warning ---
        if self.passes_final_filters(&hydrated_pool) {
            let mut writer = self.hotlist.write().unwrap();
            writer.insert(identity.address, Instant::now());
            println!("[Engine] ✅ Pool {} ajouté/mis à jour dans la hotlist.", identity.address);
        }

        Ok(())
    }

    fn update_hotlist_file(hotlist_arc: &Arc<RwLock<HashMap<Pubkey, Instant>>>) -> Result<()> {
        // ... (cette fonction reste inchangée)
        let reader = hotlist_arc.read().unwrap();

        let mut active_pools: Vec<(Pubkey, Instant)> = reader
            .iter()
            .filter(|(_, instant)| instant.elapsed().as_secs() < ACTIVITY_WINDOW_SECS)
            .map(|(k, v)| (*k, *v))
            .collect();

        active_pools.sort_by(|a, b| b.1.cmp(&a.1));

        let final_hotlist: HashSet<Pubkey> = active_pools
            .into_iter()
            .take(HOTLIST_SIZE)
            .map(|(k, _)| k)
            .collect();

        let json_data = serde_json::to_string_pretty(&final_hotlist)
            .context("Échec de la sérialisation de la hotlist en JSON")?;

        fs::write(HOTLIST_FILE_NAME, json_data)
            .context("Échec de l'écriture du fichier hotlist.json")?;

        Ok(())
    }

    // --- CORRECTION 6: Utiliser la variable pour enlever le warning ---
    fn passes_final_filters(&self, pool: &Pool) -> bool {
        let (reserve_a, reserve_b) = pool.get_reserves();
        // Exemple de filtre simple : ignorer les pools sans liquidité
        if reserve_a == 0 && reserve_b == 0 {
            println!("[Engine] ❌ Pool {} filtré car sans liquidité.", pool.address());
            return false;
        }
        true
    }

    fn decode_raw_pool(&self, address: &Pubkey, data: &[u8], owner: &Pubkey) -> Result<Pool> {
        // ... (cette fonction reste inchangée)
        use crate::decoders::*;
        match *owner {
            id if id == raydium::amm_v4::RAYDIUM_AMM_V4_PROGRAM_ID => raydium::amm_v4::decode_pool(address, data).map(Pool::RaydiumAmmV4),
            // ... Ajoutez TOUS vos autres décodeurs ici ...
            _ => Err(anyhow::anyhow!("Programme propriétaire inconnu pour le décodage : {}", owner)),
        }
    }
}