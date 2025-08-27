// src/decoders/pool_operations.rs

use anyhow::Result;
use solana_sdk::{instruction::Instruction, pubkey::Pubkey};
use solana_client::nonblocking::rpc_client::RpcClient; // <-- AJOUTER
use async_trait::async_trait; // <-- AJOUTER

#[async_trait] // <-- AJOUTER
pub trait PoolOperations {
    fn get_mints(&self) -> (Pubkey, Pubkey);
    fn get_vaults(&self) -> (Pubkey, Pubkey);

    fn address(&self) -> Pubkey;
    fn get_quote(&self, token_in_mint: &Pubkey, amount_in: u64, current_timestamp: i64) -> Result<u64>;

    // --- NOUVELLE FONCTION ---
    async fn get_quote_async(&mut self, token_in_mint: &Pubkey, amount_in: u64, rpc_client: &RpcClient) -> Result<u64>;

    fn create_swap_instruction(
        &self,
        token_in_mint: &Pubkey,          // Le mint du token qu'on donne
        amount_in: u64,                  // La quantité qu'on donne
        min_amount_out: u64,             // Le minimum qu'on accepte de recevoir
        user_accounts: &UserSwapAccounts, // Les comptes de l'utilisateur
    ) -> Result<Instruction>;
}

// Nouvelle struct pour passer les comptes de l'utilisateur de manière propre
pub struct UserSwapAccounts {
    pub owner: Pubkey,
    pub source: Pubkey,
    pub destination: Pubkey,
}