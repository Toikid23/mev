// src/bin/discover_pools.rs

use anyhow::{Result, anyhow, Context}; // <-- Ajoutez "Context" ici
use solana_client::rpc_client::RpcClient;
use solana_client::rpc_filter::{Memcmp, RpcFilterType};
use solana_sdk::pubkey::Pubkey;
use std::collections::HashSet;
use std::env;
use std::io::Write;
use std::str::FromStr;
use std::time::Duration;
use tokio;

use mev::{
    config::Config,
    data_pipeline::{
        api_connectors::{birdeye, dexscreener},
        onchain_scanner,
    },
    decoders::{raydium, Pool},
    graph_engine::Graph,
};

// --- Définition de nos stratégies de découverte ---
enum DiscoveryTask {
    Scan(ScanTarget),
    ApiTokenDiscovery(ApiTarget),
}

struct ScanTarget {
    name: &'static str,
    program_id: &'static str,
    filters: Vec<RpcFilterType>,
    decoder: fn(&Pubkey, &[u8]) -> Result<Pool>,
}

struct ApiTarget {
    name: &'static str,
    target_program_id: Pubkey,
    decoder: fn(&Pubkey, &[u8], &Pubkey) -> Result<Pool>,
}

#[tokio::main]
async fn main() -> Result<()> {
    println!("--- Lancement du Constructeur de Cache ---");
    let config = Config::load()?;
    let rpc_client = RpcClient::new(config.solana_rpc_url.clone());
    let non_blocking_rpc_client = solana_client::nonblocking::rpc_client::RpcClient::new(config.solana_rpc_url);

    const OPENBOOK_DEX_PROGRAM_ID: &str = "srmqPvymJeFKQ4zGQed1GFppgkRHL9kaELCbyksJtPX";
    let openbook_pubkey = Pubkey::from_str(OPENBOOK_DEX_PROGRAM_ID)?;
    const POOL_HYDRATION_LIMIT: usize = 100;

    let all_tasks = vec![
        DiscoveryTask::Scan(ScanTarget {
            name: "cpmm",
            program_id: "CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C",
            filters: vec![], // Géré par une logique spéciale
            decoder: |a, d| raydium::cpmm::decode_pool(a, d).map(Pool::RaydiumCpmm),
        }),
        DiscoveryTask::Scan(ScanTarget {
            name: "ammv4",
            program_id: "675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8",
            filters: vec![], // Géré par une logique spéciale
            decoder: |a, d| raydium::amm_v4::decode_pool(a, d).map(Pool::RaydiumAmmV4),
        }),
        DiscoveryTask::ApiTokenDiscovery(ApiTarget {
            name: "clmm",
            target_program_id: Pubkey::from_str("CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK")?,
            decoder: |a, d, p| raydium::clmm::decode_pool(a, d, p).map(Pool::RaydiumClmm),
        }),
    ];

    let args: Vec<String> = env::args().skip(1).collect();
    let tasks_to_run: Vec<&DiscoveryTask> = if args.is_empty() {
        println!("[Mode] Exécution de toutes les tâches de découverte.");
        all_tasks.iter().collect()
    } else {
        println!("[Mode] Exécution des tâches spécifiques : {:?}", args);
        all_tasks.iter().filter(|t| {
            let name = match t {
                DiscoveryTask::Scan(s) => s.name,
                DiscoveryTask::ApiTokenDiscovery(a) => a.name,
            };
            args.contains(&name.to_string())
        }).collect()
    };

    if tasks_to_run.is_empty() { return Ok(()); }

    let mut graph = Graph::new();

    for task in tasks_to_run {
        match task {
            DiscoveryTask::Scan(target) => {
                println!("\n--- Scan On-Chain pour {} ---", target.name);

                let raw_pools_data = match target.name {
                    "cpmm" => {
                        println!(" -> Filtre appliqué : status=0 OU status=1");
                        let filters_status_0 = vec![RpcFilterType::Memcmp(Memcmp::new_base58_encoded(321, &[0]))];
                        let filters_status_1 = vec![RpcFilterType::Memcmp(Memcmp::new_base58_encoded(321, &[1]))];
                        let mut pools_0 = onchain_scanner::find_pools_by_program_id_with_filters(&rpc_client, target.program_id, Some(filters_status_0))?;
                        let pools_1 = onchain_scanner::find_pools_by_program_id_with_filters(&rpc_client, target.program_id, Some(filters_status_1))?;
                        pools_0.extend(pools_1);
                        pools_0
                    }
                    "ammv4" => {
                        println!(" -> Filtre appliqué : status=4 OU status=6 (méthode robuste)");
                        let filters_status_4 = vec![RpcFilterType::Memcmp(Memcmp::new_base58_encoded(0, &4u64.to_le_bytes()))];
                        let filters_status_6 = vec![RpcFilterType::Memcmp(Memcmp::new_base58_encoded(0, &6u64.to_le_bytes()))];
                        let mut pools_4 = onchain_scanner::find_pools_by_program_id_with_filters(&rpc_client, target.program_id, Some(filters_status_4))?;
                        let pools_6 = onchain_scanner::find_pools_by_program_id_with_filters(&rpc_client, target.program_id, Some(filters_status_6))?;
                        pools_4.extend(pools_6);
                        pools_4
                    }
                    _ => unreachable!(),
                };

                println!("-> {} comptes bruts trouvés. Décodage...", raw_pools_data.len());

                let mut decoded_pools = Vec::new();
                for raw_pool in raw_pools_data.iter() {
                    if let Ok(pool) = (target.decoder)(&raw_pool.address, &raw_pool.data) {
                        decoded_pools.push(pool);
                    }
                }

                let decoded_count = decoded_pools.len();
                println!("-> {} pools valides décodés.", decoded_count);

                let pools_to_hydrate: Vec<Pool> = decoded_pools.into_iter().take(POOL_HYDRATION_LIMIT).collect();
                let hydration_target_count = pools_to_hydrate.len();

                println!("-> Hydratation limitée aux {} premiers pools...", hydration_target_count);
                let mut hydrated_count = 0;

                for (index, pool_to_hydrate) in pools_to_hydrate.into_iter().enumerate() {
                    print!("\r -> Hydratation {}/{}", index + 1, hydration_target_count);
                    std::io::stdout().flush()?;

                    match graph.hydrate_pool(pool_to_hydrate, &non_blocking_rpc_client).await {
                        Ok(hydrated) => {
                            graph.add_pool_to_graph(hydrated);
                            hydrated_count += 1;
                        }
                        Err(_e) => {}
                    }
                }
                println!();
                println!("-> {}/{} pools hydratés avec succès.", hydrated_count, hydration_target_count);
            }

            DiscoveryTask::ApiTokenDiscovery(target) => {
                // ... (logique pour CLMM inchangée)
                println!("\n--- Découverte par API pour {} ---", target.name);

                // PHASE 1
                const TOKENS_PER_PAGE: u32 = 50;
                const TOTAL_TOKENS_TO_FETCH: u32 = 500;
                const MIN_TOKEN_LIQUIDITY: f64 = 10000.0;
                let mut all_tokens = Vec::new();
                let mut current_offset: u32 = 0;
                println!("[Phase 1/4] Recherche des {} tokens les plus pertinents sur Birdeye...", TOTAL_TOKENS_TO_FETCH);
                while all_tokens.len() < TOTAL_TOKENS_TO_FETCH as usize {
                    match birdeye::get_top_tokens_v1(&config.birdeye_api_key, current_offset, TOKENS_PER_PAGE, MIN_TOKEN_LIQUIDITY).await {
                        Ok(new_tokens) => {
                            if new_tokens.is_empty() { break; }
                            all_tokens.extend(new_tokens);
                            current_offset += TOKENS_PER_PAGE;
                        }
                        Err(e) => { eprintln!("\nErreur Birdeye: {}. Arrêt de la pagination.", e); break; }
                    }
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
                println!(" -> Terminé. Trouvé {} tokens à analyser.", all_tokens.len());

                // PHASE 2
                println!("\n[Phase 2/4] Recherche de toutes les adresses de pools via DexScreener...");
                let mut potential_pool_addresses = HashSet::new();
                for (token_idx, token) in all_tokens.iter().enumerate() {
                    print!("\r -> Analyse du token {}/{}", token_idx + 1, all_tokens.len());
                    std::io::stdout().flush()?;
                    let found_pairs = dexscreener::search_and_filter_pairs(&token.address.to_string(), 5000.0, 5000.0).await.unwrap_or_default();
                    for pair in found_pairs {
                        potential_pool_addresses.insert(pair.address);
                    }
                }
                println!();
                println!(" -> Terminé. Trouvé {} adresses de pools uniques potentielles.", potential_pool_addresses.len());

                // PHASE 3
                println!("\n[Phase 3/4] Filtrage on-chain et décodage des pools {}...", target.name);
                let addresses_vec: Vec<Pubkey> = potential_pool_addresses.into_iter().collect();
                let mut unhydrated_pools = Vec::new();
                for chunk in addresses_vec.chunks(100) {
                    if let Ok(accounts) = rpc_client.get_multiple_accounts(chunk) {
                        for (i, account_opt) in accounts.into_iter().enumerate() {
                            if let Some(account) = account_opt {
                                if account.owner == target.target_program_id {
                                    if let Ok(pool) = (target.decoder)(&chunk[i], &account.data, &target.target_program_id) {
                                        unhydrated_pools.push(pool);
                                    }
                                }
                            }
                        }
                    }
                }
                println!(" -> Terminé. Trouvé {} pools {} valides.", unhydrated_pools.len(), target.name);

                // PHASE 4
                let pools_to_hydrate_count = unhydrated_pools.len();
                println!("\n[Phase 4/4] Hydratation des {} pools trouvés...", pools_to_hydrate_count);

                for (index, pool) in unhydrated_pools.into_iter().enumerate() {
                    print!("\r -> Hydratation {}/{}", index + 1, pools_to_hydrate_count);
                    std::io::stdout().flush()?;
                    if let Ok(hydrated) = graph.hydrate_pool(pool, &non_blocking_rpc_client).await {
                        graph.add_pool_to_graph(hydrated);
                    }
                }
                println!();
                println!(" -> Terminé.");
            }
        }
    }

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