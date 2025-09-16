#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

use anyhow::{anyhow, Result};
use mev::{
    config::Config,
    data_pipeline::manual_pools::get_manual_pool_list,
    decoders::{orca, meteora, pump, raydium, Pool},
    graph_engine::Graph,
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
                id if id == Pubkey::from_str("675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8").unwrap() => raydium::amm_v4::decode_pool(&address, &account.data).map(Pool::RaydiumAmmV4),
                id if id == Pubkey::from_str("CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C").unwrap() => raydium::cpmm::decode_pool(&address, &account.data).map(Pool::RaydiumCpmm),
                id if id == Pubkey::from_str("CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK").unwrap() => raydium::clmm::decode_pool(&address, &account.data, &id).map(Pool::RaydiumClmm),
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
    // <-- MODIFIÉ : `graph` doit être mutable pour pouvoir y ajouter des pools
    let mut graph = Graph::new();
    let total_to_hydrate = unhydrated_pools.len();
    let mut hydrated_count = 0;

    for (i, pool) in unhydrated_pools.into_iter().enumerate() {
        print!("\r -> Hydratation {}/{}", i + 1, total_to_hydrate);
        std::io::stdout().flush()?;

        // <-- MODIFIÉ : Appel de fonction statique
        match Graph::hydrate_pool(pool, &rpc_client).await {
            Ok(hydrated) => {
                // <-- MODIFIÉ : Retrait du `.await`
                graph.add_pool_to_graph(hydrated);
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
    // <-- MODIFIÉ : Accès direct à la HashMap
    if !graph.pools.is_empty() {
        // La structure est déjà simple, on peut sérialiser directement.
        let encoded_graph = bincode::serialize(&graph.pools)?;
        let mut file = std::fs::File::create("graph_cache.bin")?;
        file.write_all(&encoded_graph)?;
        println!("-> Graphe sauvegardé avec {} pools dans 'graph_cache.bin'.", graph.pools.len());
    } else {
        println!("[AVERTISSEMENT] Le graphe est vide, aucun fichier de cache n'a été créé.");
    }

    Ok(())
}