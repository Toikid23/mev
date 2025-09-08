// DANS : src/bin/census_runner.rs

use anyhow::Result;
use mev::{config::Config, filtering::census, rpc::ResilientRpcClient}; // <-- MODIFIÉ

#[tokio::main]
async fn main() -> Result<()> {
    // 1. Charger la configuration et créer notre client résilient
    let config = Config::load()?;
    let rpc_client = ResilientRpcClient::new(config.solana_rpc_url, 3, 500);

    // 2. Lancer la fonction principale du recensement (maintenant avec await)
    census::run_census(&rpc_client).await?;

    println!("\n✅ Recensement terminé. Le fichier 'pools_universe.json' a été créé ou mis à jour.");

    Ok(())
}