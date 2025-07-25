// src/bin/dev_runner.rs

// On importe le module raydium que nous voulons tester.
use mev::data_pipeline::discovery::raydium;
use anyhow::Result;

// La fonction fetch_raydium_pools est asynchrone, nous devons donc
// utiliser la macro `#[tokio::main]` pour que notre fonction `main`
// puisse utiliser `.await`.
#[tokio::main]
async fn main() -> Result<()> {
    println!("--- Development Runner: Test de la récupération complète des pools Raydium ---");

    // On appelle notre fonction et on attend le résultat.
    // On utilise un `match` pour gérer proprement le cas de succès et le cas d'erreur.
    match raydium::fetch_raydium_pools().await {
        Ok(pools) => {
            println!("\n✅ --- Test de récupération terminé avec succès ! ---");
            println!("Nombre total de pools Raydium effectivement récupérés: {}", pools.len());

            // On affiche le détail des 3 premiers pools pour vérifier que les données sont bien là.
            println!("\n--- Aperçu des 3 premiers pools : ---");
            for (i, pool) in pools.iter().take(3).enumerate() {
                println!("\n--- Pool N°{} ---", i + 1);
                println!("  ID:         {}", pool.id);
                println!("  Type:       {}", pool.pool_type);
                println!("  Mint A:     {}", pool.mint_a.address);
                println!("  Mint B:     {}", pool.mint_b.address);
                println!("  TVL:        ${:.2}", pool.tvl); // Formatage pour la lisibilité
                println!("  Volume 24h: ${:.2}", pool.day.volume);
            }
        },
        Err(e) => {
            println!("\n❌ --- Échec du test de récupération. ---");
            println!("Erreur: {}", e);
        }
    }

    Ok(())
}