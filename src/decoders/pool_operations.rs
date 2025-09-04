// src/decoders/pool_operations.rs

use anyhow::Result;
use solana_sdk::{instruction::Instruction, pubkey::Pubkey};
use solana_client::nonblocking::rpc_client::RpcClient;
use async_trait::async_trait;

#[async_trait]
pub trait PoolOperations {

    fn get_mints(&self) -> (Pubkey, Pubkey);

    fn get_vaults(&self) -> (Pubkey, Pubkey);

    fn get_reserves(&self) -> (u64, u64);

    fn address(&self) -> Pubkey;

    fn get_quote(&self, token_in_mint: &Pubkey, amount_in: u64, current_timestamp: i64) -> Result<u64>;

    fn get_required_input(
        &mut self,
        token_out_mint: &Pubkey, // Le mint du token que l'on veut recevoir
        amount_out: u64,         // La quantité que l'on veut recevoir
        current_timestamp: i64
    ) -> Result<u64>;

    fn create_swap_instruction(
        &self,
        token_in_mint: &Pubkey,
        amount_in: u64,
        min_amount_out: u64,
        user_accounts: &UserSwapAccounts,
    ) -> Result<Instruction>;
}

// La struct reste inchangée
pub struct UserSwapAccounts {
    pub owner: Pubkey,
    pub source: Pubkey,
    pub destination: Pubkey,
}