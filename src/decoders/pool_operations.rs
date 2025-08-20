// src/decoders/pool_operations.rs

use anyhow::Result;
use solana_sdk::pubkey::Pubkey;
use solana_client::nonblocking::rpc_client::RpcClient; // <-- AJOUTER
use async_trait::async_trait; // <-- AJOUTER

#[async_trait] // <-- AJOUTER
pub trait PoolOperations {
    fn get_mints(&self) -> (Pubkey, Pubkey);
    fn get_vaults(&self) -> (Pubkey, Pubkey);
    fn get_quote(&self, token_in_mint: &Pubkey, amount_in: u64, current_timestamp: i64) -> Result<u64>;

    // --- NOUVELLE FONCTION ---
    async fn get_quote_async(&mut self, token_in_mint: &Pubkey, amount_in: u64, rpc_client: &RpcClient) -> Result<u64>;
}