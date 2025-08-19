// src/data_pipeline/onchain_scanner.rs

use anyhow::Result;
use solana_client::rpc_client::RpcClient;
use solana_client::rpc_config::{RpcAccountInfoConfig, RpcProgramAccountsConfig};
use solana_client::rpc_filter::RpcFilterType;
use solana_account_decoder::UiAccountEncoding;
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;

#[derive(Debug)]
pub struct RawPoolData {
    pub address: Pubkey,
    pub data: Vec<u8>,
}

/// Trouve tous les comptes appartenant à un programme donné, avec la possibilité d'appliquer
/// des filtres au niveau du nœud RPC pour réduire le nombre de résultats.
///
/// # Arguments
/// * `rpc_client` - Le client RPC bloquant pour communiquer avec le nœud.
/// * `program_id_str` - L'adresse du programme à scanner.
/// * `filters` - Une `Option` contenant un `Vec` de `RpcFilterType`. Si `None`, aucun filtre n'est appliqué.
pub fn find_pools_by_program_id_with_filters(
    rpc_client: &RpcClient,
    program_id_str: &str,
    filters: Option<Vec<RpcFilterType>>,
) -> Result<Vec<RawPoolData>> {
    println!(
        "\nLancement du scan on-chain pour le programme : {}",
        program_id_str
    );

    let program_id = Pubkey::from_str(program_id_str)?;

    let account_config = RpcAccountInfoConfig {
        encoding: Some(UiAccountEncoding::Base64),
        data_slice: None,
        commitment: None,
        min_context_slot: None,
    };

    // --- LA CORRECTION EST ICI ---
    let config = RpcProgramAccountsConfig {
        filters,
        account_config,
        with_context: Some(false),
        sort_results: None, // Le champ manquant a été ajouté.
    };

    let accounts = rpc_client.get_program_accounts_with_config(&program_id, config)?;

    println!(
        "-> Scan terminé. {} comptes trouvés pour ce programme avec les filtres appliqués.",
        accounts.len()
    );

    let raw_pools = accounts
        .into_iter()
        .map(|(address, account)| RawPoolData {
            address,
            data: account.data,
        })
        .collect();

    Ok(raw_pools)
}