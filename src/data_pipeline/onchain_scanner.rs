// DANS : src/data_pipeline/onchain_scanner.rs

use anyhow::Result;
use solana_client::rpc_config::{RpcAccountInfoConfig, RpcProgramAccountsConfig};
use solana_client::rpc_filter::RpcFilterType;
use solana_account_decoder::UiAccountEncoding;
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;
use crate::rpc::ResilientRpcClient; // <-- NOUVEL IMPORT
use tracing::{error, info, warn, debug};


#[derive(Debug)]
pub struct RawPoolData {
    pub address: Pubkey,
    pub data: Vec<u8>,
}

#[derive(Debug)]
pub struct RawPoolDataWithOwner {
    pub address: Pubkey,
    pub data: Vec<u8>,
    pub owner: Pubkey, // On ajoute l'owner ici
}

// La fonction devient `async` et prend notre client résilient
pub async fn find_pools_by_program_id_with_filters(
    rpc_client: &ResilientRpcClient,
    program_id_str: &str,
    filters: Option<Vec<RpcFilterType>>,
) -> Result<Vec<RawPoolData>> {
    info!(program_id = program_id_str, "Lancement du scan on-chain.");

    let program_id = Pubkey::from_str(program_id_str)?;

    let account_config = RpcAccountInfoConfig {
        encoding: Some(UiAccountEncoding::Base64),
        data_slice: None,
        commitment: None,
        min_context_slot: None,
    };

    let config = RpcProgramAccountsConfig {
        filters,
        account_config,
        with_context: Some(false),
        sort_results: None,
    };

    // L'appel est maintenant asynchrone avec `.await`
    let accounts = rpc_client
        .get_program_accounts_with_config(&program_id, config)
        .await?;

    info!(account_count = accounts.len(), "Scan on-chain terminé.");

    let raw_pools = accounts
        .into_iter()
        .map(|(address, account)| RawPoolData {
            address,
            data: account.data,
        })
        .collect();

    Ok(raw_pools)
}