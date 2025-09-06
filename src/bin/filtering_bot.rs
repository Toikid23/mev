// DANS : src/bin/filtering_bot.rs

use anyhow::Result;
use mev::{
    config::Config,
    filtering::{cache::PoolCache, engine::FilteringEngine, watcher::Watcher},
};
use tokio::sync::mpsc;

#[tokio::main]
async fn main() -> Result<()> {
    println!("--- Démarrage du Bot de Filtrage (Architecture Finale) ---");

    // 1. Charger la configuration
    let config = Config::load()?;
    let geyser_url = "http://127.0.0.1:10000".to_string(); // À mettre dans le .env plus tard

    // 2. Charger la "bibliothèque" de pools en mémoire
    let cache = PoolCache::load()?;
    if cache.pools.is_empty() {
        println!("[AVERTISSEMENT] Le cache est vide. Lancez 'census_runner' au moins une fois.");
    }

    // 3. Créer le canal de communication
    let (active_pool_sender, active_pool_receiver) = mpsc::channel(2048);

    // 4. Initialiser et lancer le Watcher dans une tâche d'arrière-plan
    let watcher = Watcher::new(geyser_url);
    tokio::spawn(async move {
        // Le cache est déplacé dans le watcher qui n'en a plus besoin ensuite.
        if let Err(e) = watcher.run(cache, active_pool_sender).await {
            eprintln!("[ERREUR CRITIQUE] Le Watcher Geyser s'est arrêté : {:?}", e);
        }
    });

    // 5. Initialiser et lancer le Moteur d'Analyse
    // Le moteur prend le contrôle du thread principal pour écouter les messages.
    let initial_cache_for_engine = PoolCache::load()?; // Le moteur a aussi besoin de sa copie du cache.
    let engine = FilteringEngine::new(config.solana_rpc_url);
    engine.run(active_pool_receiver, initial_cache_for_engine).await;

    Ok(())
}