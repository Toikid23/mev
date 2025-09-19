// DANS : src/filtering/census.rs (VERSION CORRIGÉE ET COMPLÈTE)

use crate::data_pipeline::onchain_scanner;
use crate::rpc::ResilientRpcClient;
use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use solana_sdk::pubkey::Pubkey;
use std::collections::HashMap;
use std::fs::File;
use std::str::FromStr;
use tracing::{error, info, warn, debug};

// Structure pour la sérialisation JSON
#[derive(serde::Serialize)]
struct PoolDataForJson {
    owner: String,
    data_b64: String,
}

/// Scanne un programme DEX et retourne une liste de comptes bruts avec leur propriétaire.
async fn scan_program_for_pools(
    rpc_client: &ResilientRpcClient,
    program_id: Pubkey,
) -> Result<Vec<onchain_scanner::RawPoolData>> {
    info!(program_id = %program_id, "Scan du programme.");
    onchain_scanner::find_pools_by_program_id_with_filters(
        rpc_client,
        &program_id.to_string(),
        None,
    )
        .await
}

/// Fonction principale du recensement.
pub async fn run_census(rpc_client: &ResilientRpcClient) -> Result<()> {
    info!("--- Démarrage du recensement complet ---");


    // Une seule map pour agréger tous les résultats
    let mut raw_data_map: HashMap<String, PoolDataForJson> = HashMap::new();

    let programs_to_scan = vec![
        "675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8", // Raydium AMM V4
        "CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C", // Raydium CPMM
        "CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK", // Raydium CLMM
        "whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc", // Orca Whirlpool
        "Eo7WjKq67rjJQSZxS6z3YkapzY3eMj6Xy8X5EQVn5UaB", // Meteora DAMM V1
        "cpamdpZCGKUy5JxQXB4dcpGPiikHawvSWAd6mEn1sGG",  // Meteora DAMM V2
        "LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo",  // Meteora DLMM
        "pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA",  // Pump AMM
    ];

    for program_str in programs_to_scan {
        let program_id = Pubkey::from_str(program_str)?;
        match scan_program_for_pools(rpc_client, program_id).await {
            Ok(pools) => {
                info!(program_id = %program_id, count = pools.len(), "Scan du programme terminé.");
                for raw_pool in pools {
                    raw_data_map.insert(
                        raw_pool.address.to_string(),
                        PoolDataForJson {
                            owner: program_id.to_string(),
                            data_b64: STANDARD.encode(&raw_pool.data),
                        },
                    );
                }
            }
            Err(e) => {
                error!("[Recensement] ⚠️ Erreur lors du scan du programme {}: {}", program_id, e);
            }
        }
    }

    info!(total_count = raw_data_map.len(), "Scan de tous les programmes terminé.");

    // Sauvegarde dans le fichier
    let file = File::create("pools_universe.json")?; // Nom de fichier simplifié
    serde_json::to_writer_pretty(file, &raw_data_map)
        .context("Échec de la sauvegarde du cache de l'univers des pools")?;

    match std::fs::metadata("pools_universe.json") {
        Ok(metadata) => {
            let file_size_mb = metadata.len() as f64 / (1024.0 * 1024.0);
            info!(
            pool_count = raw_data_map.len(),
            file_size_mb = format!("{:.2}", file_size_mb),
            "Sauvegarde de l'univers des pools terminée."
        );
        }
        Err(e) => {
            warn!(error = %e, "Sauvegarde de l'univers des pools terminée, mais impossible de lire la taille du fichier.");
        }
    }

    Ok(())
}