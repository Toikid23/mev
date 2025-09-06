// DANS : src/bin/census_runner.rs

use anyhow::Result;
use mev::{config::Config, filtering::census};
use solana_client::rpc_client::RpcClient;

fn main() -> Result<()> {
    // 1. Charger la configuration (URL du RPC, etc.)
    let config = Config::load()?;
    let rpc_client = RpcClient::new(config.solana_rpc_url);

    // 2. Lancer la fonction principale du recensement
    census::run_census(&rpc_client)?;

    println!("\n✅ Recensement terminé. Le fichier 'pools_universe.json' a été créé ou mis à jour.");

    Ok(())
}