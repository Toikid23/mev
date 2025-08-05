// DANS : src/decoders/pool_operations.rs

use anyhow::Result;
use solana_sdk::pubkey::Pubkey;

pub trait PoolOperations {
    fn get_mints(&self) -> (Pubkey, Pubkey);
    fn get_vaults(&self) -> (Pubkey, Pubkey);

    // CORRECTION : Ajout du paramètre `current_timestamp` ici
    fn get_quote(&self, token_in_mint: &Pubkey, amount_in: u64, current_timestamp: i64) -> Result<u64>;
}