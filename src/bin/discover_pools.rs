// src/bin/discover_pools.rs

use anyhow::{Result, anyhow};
use solana_client::rpc_client::RpcClient;
use solana_client::rpc_filter::{Memcmp, RpcFilterType};
use solana_sdk::pubkey::Pubkey;
use std::env;
use std::io::Write;
use std::str::FromStr;
use tokio::runtime::Runtime;

use mev::{
    config::Config,
    data_pipeline::onchain_scanner,
    decoders::{raydium, Pool},
    graph_engine::Graph,
};

// --- Définition de ce qu'est une "tâche" de découverte ---
enum DiscoveryTask {
    Scan(ScanTarget),
    LoadManual(ManualTarget),
}

struct ScanTarget {
    name: &'static str,
    program_id: &'static str,
    filters: Vec<RpcFilterType>,
    decoder: fn(&Pubkey, &[u8]) -> Result<Pool>,
}

struct ManualTarget {
    name: &'static str,
    program_id: Pubkey,
    addresses: Vec<&'static str>,
    decoder: fn(&Pubkey, &[u8], &Pubkey) -> Result<Pool>,
}

fn main() -> Result<()> {
    println!("--- Lancement du Constructeur de Cache ---");
    let config = Config::load()?;
    let rpc_client = RpcClient::new(config.solana_rpc_url);
    let rt = Runtime::new()?;
    let non_blocking_rpc_client = solana_client::nonblocking::rpc_client::RpcClient::new(rpc_client.url());

    // --- Définition de TOUTES les tâches possibles ---
    let all_tasks = vec![
        DiscoveryTask::Scan(ScanTarget {
            name: "cpmm",
            program_id: "CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C",
            filters: vec![RpcFilterType::Memcmp(Memcmp::new_base58_encoded(201, b"1"))],
            decoder: |a, d| raydium::cpmm::decode_pool(a, d).map(Pool::RaydiumCpmm),
        }),
        DiscoveryTask::Scan(ScanTarget {
            name: "ammv4",
            program_id: "675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8",
            filters: vec![RpcFilterType::Memcmp(Memcmp::new_raw_bytes(0, 4u64.to_le_bytes().to_vec()))],
            decoder: |a, d| raydium::amm_v4::decode_pool(a, d).map(Pool::RaydiumAmmV4),
        }),
        // --- TÂCHE MANUELLE POUR LE CLMM ---
        DiscoveryTask::LoadManual(ManualTarget {
            name: "clmm",
            program_id: Pubkey::from_str("CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK")?,
            addresses: vec![
                "YrrUStgPugDp8BbfosqDeFssen6sA75ZS1QJvgnHtmY", // SOL-USDC
                "5y8t3T2d4R45JV32xH2e22jUaKj25i5d2g1xYrWEg6K7", // RAY-SOL
                "Db2PCqNpXHtTtXPLs23hDHRFVcvoa1TCf6g72yG2j1g7", // mSOL-SOL
                "3HDEKZnpt4wTgy8yP3mLEvBxVo3d213z2Y8Gv3yUKBJk", // JITO-SOL
            ],
            decoder: |a, d, p| raydium::clmm::decode_pool(a, d, p).map(Pool::RaydiumClmm),
        }),
    ];

    let args: Vec<String> = env::args().skip(1).collect();
    let tasks_to_run: Vec<&DiscoveryTask> = if args.is_empty() {
        all_tasks.iter().collect()
    } else {
        all_tasks.iter().filter(|t| {
            let name = match t {
                DiscoveryTask::Scan(s) => s.name,
                DiscoveryTask::LoadManual(m) => m.name,
            };
            args.contains(&name.to_string())
        }).collect()
    };

    if tasks_to_run.is_empty() {
        println!("[AVERTISSEMENT] Aucun protocole valide trouvé dans les arguments. Rien à faire.");
        return Ok(());
    }

    let mut graph = Graph::new();

    for task in tasks_to_run {
        match task {
            DiscoveryTask::Scan(target) => {
                println!("\n--- Scan pour {} ---", target.name);
                let raw_pools_data = onchain_scanner::find_pools_by_program_id_with_filters(&rpc_client, target.program_id, Some(target.filters.clone()))?;
                println!("-> {} pools trouvés. Hydratation...", raw_pools_data.len());
                for (index, raw_pool) in raw_pools_data.iter().enumerate() {
                    if let Ok(decoded_pool) = (target.decoder)(&raw_pool.address, &raw_pool.data) {
                        if let Ok(hydrated) = rt.block_on(graph.hydrate_pool(decoded_pool, &non_blocking_rpc_client)) {
                            graph.add_pool_to_graph(hydrated);
                        }
                    }
                    print!("\r -> Pools traités : {}/{}", index + 1, raw_pools_data.len());
                    std::io::stdout().flush()?;
                }
                println!(); // Nouvelle ligne
            }
            DiscoveryTask::LoadManual(target) => {
                println!("\n--- Chargement manuel pour {} ---", target.name);
                for (index, address_str) in target.addresses.iter().enumerate() {
                    let address = Pubkey::from_str(address_str)?;
                    if let Ok(data) = rpc_client.get_account_data(&address) {
                        if let Ok(decoded_pool) = (target.decoder)(&address, &data, &target.program_id) {
                            if let Ok(hydrated) = rt.block_on(graph.hydrate_pool(decoded_pool, &non_blocking_rpc_client)) {
                                graph.add_pool_to_graph(hydrated);
                            }
                        }
                    }
                    print!("\r -> Pools traités : {}/{}", index + 1, target.addresses.len());
                    std::io::stdout().flush()?;
                }
                println!(); // Nouvelle ligne
            }
        }
    }

    // Sauvegarde du cache
    println!("\n--- Sauvegarde du Graphe ---");
    if !graph.pools.is_empty() {
        let encoded_graph = bincode::serialize(&graph.pools)?;
        let mut file = std::fs::File::create("graph_cache.bin")?;
        file.write_all(&encoded_graph)?;
        println!("-> Graphe sauvegardé avec {} pools.", graph.pools.len());
    } else {
        println!("[AVERTISSEMENT] Le graphe est vide.");
    }

    Ok(())
}