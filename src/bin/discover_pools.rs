// DANS : src/bin/discover_pools.rs

use anyhow::{anyhow, Result};
use mev::{
    config::Config,
    data_pipeline::manual_pools::get_manual_pool_list,
    decoders::{orca, meteora, pump, raydium, Pool},
    graph_engine::Graph, // On importe la nouvelle structure
    rpc::ResilientRpcClient,
};
use solana_sdk::pubkey::Pubkey;
use std::io::Write;
use std::str::FromStr;

#[tokio::main]
async fn main() -> Result<()> {
    println!("--- Lancement du Constructeur de Cache (Mode Manuel) ---");
    let config = Config::load()?;
    let rpc_client = ResilientRpcClient::new(config.solana_rpc_url, 3, 500);

    let manual_pool_addresses_str = get_manual_pool_list();
    if manual_pool_addresses_str.is_empty() {
        println!("[AVERTISSEMENT] La liste de pools manuelle est vide. Rien à faire.");
        return Ok(());
    }
    println!("[1/4] Chargement de {} adresses depuis la liste manuelle.", manual_pool_addresses_str.len());

    let pool_pubkeys: Vec<Pubkey> = manual_pool_addresses_str.iter().map(|s| Pubkey::from_str(s)).collect::<Result<_, _>>()?;

    println!("[2/4] Récupération des données des comptes on-chain...");
    let accounts_data = rpc_client.get_multiple_accounts(&pool_pubkeys).await?;

    println!("[3/4] Identification et décodage des pools...");
    let mut unhydrated_pools = Vec::new();
    for (index, account_opt) in accounts_data.into_iter().enumerate() {
        let address = pool_pubkeys[index];
        if let Some(account) = account_opt {
            let decoded_pool_result = match account.owner {
                // ... (toute votre logique de `match` reste la même)
                id if id == Pubkey::from_str("675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8").unwrap() || id == Pubkey::from_str("DRaya7Kj3aMWQSy19kSjvmuwq9docCHofyP9kanQGaav").unwrap() => raydium::amm_v4::decode_pool(&address, &account.data).map(Pool::RaydiumAmmV4),
                id if id == Pubkey::from_str("CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C").unwrap() || id == Pubkey::from_str("DRaycpLY18LhpbydsBWbVJtxpNv9oXPgjRSfpF2bWpYb").unwrap() => raydium::cpmm::decode_pool(&address, &account.data).map(Pool::RaydiumCpmm),
                id if id == Pubkey::from_str("CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK").unwrap() || id == Pubkey::from_str("DRayAUgENGQBKVaX8owNhgzkEDyoHTGVEGHVJT1E9pfH").unwrap() => raydium::clmm::decode_pool(&address, &account.data, &id).map(Pool::RaydiumClmm),
                id if id == Pubkey::from_str("Eo7WjKq67rjJQSZxS6z3YkapzY3eMj6Xy8X5EQVn5UaB").unwrap() => meteora::damm_v1::decode_pool(&address, &account.data).map(Pool::MeteoraDammV1),
                id if id == Pubkey::from_str("cpamdpZCGKUy5JxQXB4dcpGPiikHawvSWAd6mEn1sGG").unwrap() => meteora::damm_v2::decode_pool(&address, &account.data).map(Pool::MeteoraDammV2),
                id if id == Pubkey::from_str("LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo").unwrap() => meteora::dlmm::decode_lb_pair(&address, &account.data, &account.owner).map(Pool::MeteoraDlmm),
                id if id == Pubkey::from_str("whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc").unwrap() => orca::whirlpool::decode_pool(&address, &account.data).map(Pool::OrcaWhirlpool),
                id if id == Pubkey::from_str("pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA").unwrap() => pump::amm::decode_pool(&address, &account.data).map(Pool::PumpAmm),
                _ => Err(anyhow!("Programme propriétaire inconnu: {}", account.owner)),
            };
            if let Ok(pool) = decoded_pool_result {
                unhydrated_pools.push(pool);
            }
        }
    }
    println!(" -> {} pools valides décodés.", unhydrated_pools.len());

    println!("\n[4/4] Hydratation des pools décodés...");
    let graph = Graph::new(); // On crée notre nouvelle structure Graph
    let total_to_hydrate = unhydrated_pools.len();
    let mut hydrated_count = 0;

    for (i, pool) in unhydrated_pools.into_iter().enumerate() {
        print!("\r -> Hydratation {}/{}", i + 1, total_to_hydrate);
        std::io::stdout().flush()?;
        match graph.hydrate_pool(pool, &rpc_client).await {
            Ok(hydrated) => {
                graph.add_pool_to_graph(hydrated).await; // L'ajout est maintenant `async`
                hydrated_count += 1;
            }
            Err(e) => {
                println!("\n[ERREUR] Échec de l'hydratation pour un pool: {}", e);
            }
        }
    }
    println!();
    println!(" -> {}/{} pools hydratés avec succès.", hydrated_count, total_to_hydrate);

    println!("\n--- Sauvegarde du Graphe ---");
    // --- LA CORRECTION EST ICI ---
    let pools_reader = graph.pools.read().await;
    if !pools_reader.is_empty() {
        // 1. On crée une structure simple (sans verrous) juste pour la sérialisation.
        let mut serializable_pools = std::collections::HashMap::new();
        for (key, pool_arc_rwlock) in pools_reader.iter() {
            let pool_data = pool_arc_rwlock.read().await;
            serializable_pools.insert(*key, (*pool_data).clone());
        }

        // 2. On sérialise cette nouvelle map simple.
        let encoded_graph = bincode::serialize(&serializable_pools)?;
        let mut file = std::fs::File::create("graph_cache.bin")?;
        file.write_all(&encoded_graph)?;
        println!("-> Graphe sauvegardé avec {} pools dans 'graph_cache.bin'.", serializable_pools.len());
    } else {
        println!("[AVERTISSEMENT] Le graphe est vide, aucun fichier de cache n'a été créé.");
    }

    Ok(())
}