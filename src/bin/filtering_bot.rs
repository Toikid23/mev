#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

use anyhow::Result;
use mev::{
    config::Config,
    filtering::{cache::PoolCache, engine::FilteringEngine, watcher::Watcher},
    decoders::{raydium, meteora, pump}, // <-- Importer les décodeurs pour les program_id
};
use solana_sdk::pubkey::Pubkey; // <-- Importer Pubkey
use std::str::FromStr; // <-- Importer FromStr pour convertir les strings en Pubkey
use tokio::sync::mpsc;

#[tokio::main]
async fn main() -> Result<()> {
    println!("--- Démarrage du Bot de Filtrage (Architecture Finale) ---");

    let config = Config::load()?;
    let geyser_url = "http://127.0.0.1:10000".to_string();

    let cache = PoolCache::load()?;
    if cache.pools.is_empty() {
        println!("[AVERTISSEMENT] Le cache est vide. Lancez 'census_runner' au moins une fois.");
    }

    // --- NOUVEAU BLOC : Définir les programmes à surveiller ---
    let programs_to_watch = vec![
        raydium::amm_v4::RAYDIUM_AMM_V4_PROGRAM_ID,
        Pubkey::from_str("CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C")?, // Raydium CPMM
        Pubkey::from_str("CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK")?, // Raydium CLMM
        Pubkey::from_str("whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc")?, // Orca Whirlpool
        Pubkey::from_str("Eo7WjKq67rjJQSZxS6z3YkapzY3eMj6Xy8X5EQVn5UaB")?, // Meteora DAMM v1
        Pubkey::from_str("cpamdpZCGKUy5JxQXB4dcpGPiikHawvSWAd6mEn1sGG")?, // Meteora DAMM v2
        meteora::dlmm::PROGRAM_ID, // Meteora DLMM
        pump::amm::PUMP_PROGRAM_ID,
    ];
    // --- FIN DU BLOC ---

    let (active_pool_sender, active_pool_receiver) = mpsc::channel(2048);

    let watcher = Watcher::new(geyser_url);
    tokio::spawn(async move {
        // --- MODIFICATION DE L'APPEL ---
        if let Err(e) = watcher.run(cache, programs_to_watch, active_pool_sender).await {
            eprintln!("[ERREUR CRITIQUE] Le Watcher Geyser s'est arrêté : {:?}", e);
        }
    });

    let initial_cache_for_engine = PoolCache::load()?;
    let engine = FilteringEngine::new(config.solana_rpc_url);
    engine.run(active_pool_receiver, initial_cache_for_engine).await;

    Ok(())
}