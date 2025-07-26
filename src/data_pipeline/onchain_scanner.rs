// src/data_pipeline/onchain_scanner.rs

use anyhow::Result;
use solana_client::rpc_client::RpcClient;
use solana_client::rpc_config::{RpcAccountInfoConfig, RpcProgramAccountsConfig};
use solana_account_decoder::UiAccountEncoding;
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;

#[derive(Debug)]
pub struct RawPoolData {
    pub address: Pubkey,
    pub data: Vec<u8>,
}

pub fn find_pools_by_program_id(
    rpc_client: &RpcClient,
    program_id_str: &str,
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

    let config = RpcProgramAccountsConfig {
        filters: None,
        account_config,
        with_context: Some(false),
        sort_results: None,
    };

    let accounts = rpc_client.get_program_accounts_with_config(&program_id, config)?;

    println!(
        "-> Scan terminé. {} comptes trouvés pour ce programme.",
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