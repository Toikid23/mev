use solana_sdk::pubkey::Pubkey;
use anyhow::Result;
use serde::{Serialize, Deserialize};
use async_trait::async_trait;
use solana_sdk::instruction::Instruction;


// --- 1. Déclarer tous nos modules principaux ---
pub mod pool_operations;
pub mod raydium;
pub mod orca;
pub mod meteora;
pub mod spl_token_decoders;
pub mod pump;
pub mod pool_factory;

// --- 2. Importer le trait ---
pub use pool_operations::PoolOperations;
pub use pool_factory::PoolFactory;

// --- 3. Définir l'enum unifié avec les BONS NOMS ---
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Pool {
    RaydiumAmmV4(raydium::amm_v4::DecodedAmmPool),
    RaydiumCpmm(raydium::cpmm::DecodedCpmmPool),
    RaydiumClmm(raydium::clmm::DecodedClmmPool),
    MeteoraDammV1(meteora::damm_v1::DecodedMeteoraSbpPool),
    MeteoraDammV2(meteora::damm_v2::DecodedMeteoraDammPool),
    MeteoraDlmm(meteora::dlmm::DecodedDlmmPool),
    OrcaWhirlpool(orca::whirlpool::DecodedWhirlpoolPool),
    PumpAmm(pump::amm::DecodedPumpAmmPool),
}

// --- 4. Implémenter le trait pour l'enum avec les BONS NOMS ---
#[async_trait]
impl PoolOperations for Pool {
    fn get_mints(&self) -> (Pubkey, Pubkey) {
        match self {
            Pool::RaydiumAmmV4(p) => p.get_mints(),
            Pool::RaydiumCpmm(p) => p.get_mints(),
            Pool::RaydiumClmm(p) => p.get_mints(),
            Pool::MeteoraDammV1(p) => p.get_mints(), // <-- LA CORRECTION EST ICI
            Pool::MeteoraDammV2(p) => p.get_mints(),
            Pool::MeteoraDlmm(p) => p.get_mints(),
            Pool::OrcaWhirlpool(p) => p.get_mints(),
            Pool::PumpAmm(p) => p.get_mints(),
        }
    }

    fn get_vaults(&self) -> (Pubkey, Pubkey) {
        match self {
            Pool::RaydiumAmmV4(p) => p.get_vaults(),
            Pool::RaydiumCpmm(p) => p.get_vaults(),
            Pool::RaydiumClmm(p) => p.get_vaults(),
            Pool::MeteoraDammV1(p) => p.get_vaults(), // <-- LA CORRECTION EST ICI
            Pool::MeteoraDammV2(p) => p.get_vaults(),
            Pool::MeteoraDlmm(p) => p.get_vaults(),
            Pool::OrcaWhirlpool(p) => p.get_vaults(),
            Pool::PumpAmm(p) => p.get_vaults(),
        }
    }

    fn get_reserves(&self) -> (
        u64, u64) {
        match self {
            Pool::RaydiumAmmV4(p) => p.get_reserves(),
            Pool::RaydiumCpmm(p) => p.get_reserves(),
            Pool::RaydiumClmm(p) => p.get_reserves(),
            Pool::MeteoraDammV1(p) => p.get_reserves(),
            Pool::MeteoraDammV2(p) => p.get_reserves(),
            Pool::MeteoraDlmm(p) => p.get_reserves(),
            Pool::OrcaWhirlpool(p) => p.get_reserves(),
            Pool::PumpAmm(p) => p.get_reserves(),
        }
    }

    fn address(&self) -> Pubkey {
        match self {
            Pool::RaydiumAmmV4(p) => p.address,
            Pool::RaydiumCpmm(p) => p.address,
            Pool::RaydiumClmm(p) => p.address,
            Pool::MeteoraDammV1(p) => p.address,
            Pool::MeteoraDammV2(p) => p.address,
            Pool::MeteoraDlmm(p) => p.address,
            Pool::OrcaWhirlpool(p) => p.address,
            Pool::PumpAmm(p) => p.address,
        }
    }


    fn get_quote_with_details(&self, token_in_mint: &Pubkey, amount_in: u64, current_timestamp: i64) -> Result<pool_operations::QuoteResult> {
        match self {
            Pool::RaydiumAmmV4(p) => p.get_quote_with_details(token_in_mint, amount_in, current_timestamp),
            Pool::RaydiumCpmm(p) => p.get_quote_with_details(token_in_mint, amount_in, current_timestamp),
            Pool::RaydiumClmm(p) => p.get_quote_with_details(token_in_mint, amount_in, current_timestamp),
            Pool::MeteoraDammV1(p) => p.get_quote_with_details(token_in_mint, amount_in, current_timestamp),
            Pool::MeteoraDammV2(p) => p.get_quote_with_details(token_in_mint, amount_in, current_timestamp),
            Pool::MeteoraDlmm(p) => p.get_quote_with_details(token_in_mint, amount_in, current_timestamp),
            Pool::OrcaWhirlpool(p) => p.get_quote_with_details(token_in_mint, amount_in, current_timestamp),
            Pool::PumpAmm(p) => p.get_quote_with_details(token_in_mint, amount_in, current_timestamp),
        }
    }

    fn get_required_input(&mut self, token_out_mint: &Pubkey, amount_out: u64, current_timestamp: i64) -> Result<u64> {
        match self {
            Pool::RaydiumAmmV4(p) => p.get_required_input(token_out_mint, amount_out, current_timestamp),
            Pool::RaydiumCpmm(p) => p.get_required_input(token_out_mint, amount_out, current_timestamp),
            Pool::RaydiumClmm(p) => p.get_required_input(token_out_mint, amount_out, current_timestamp),
            Pool::MeteoraDammV1(p) => p.get_required_input(token_out_mint, amount_out, current_timestamp),
            Pool::MeteoraDammV2(p) => p.get_required_input(token_out_mint, amount_out, current_timestamp),
            Pool::MeteoraDlmm(p) => p.get_required_input(token_out_mint, amount_out, current_timestamp),
            Pool::OrcaWhirlpool(p) => p.get_required_input(token_out_mint, amount_out, current_timestamp),
            Pool::PumpAmm(p) => p.get_required_input(token_out_mint, amount_out, current_timestamp),
        }
    }


    fn update_from_account_data(&mut self, account_pubkey: &Pubkey, account_data: &[u8]) -> Result<()> {
        match self {
            Pool::RaydiumAmmV4(p) => p.update_from_account_data(account_pubkey, account_data),
            Pool::RaydiumCpmm(p) => p.update_from_account_data(account_pubkey, account_data),
            Pool::RaydiumClmm(p) => p.update_from_account_data(account_pubkey, account_data),
            Pool::MeteoraDammV1(p) => p.update_from_account_data(account_pubkey, account_data),
            Pool::MeteoraDammV2(p) => p.update_from_account_data(account_pubkey, account_data),
            Pool::MeteoraDlmm(p) => p.update_from_account_data(account_pubkey, account_data),
            Pool::OrcaWhirlpool(p) => p.update_from_account_data(account_pubkey, account_data),
            Pool::PumpAmm(p) => p.update_from_account_data(account_pubkey, account_data),
        }
    }


    fn create_swap_instruction(
        &self,
        token_in_mint: &Pubkey,
        amount_in: u64,
        min_amount_out: u64,
        user_accounts: &pool_operations::UserSwapAccounts, // Utiliser le chemin complet pour éviter l'ambiguïté
    ) -> Result<Instruction> {
        match self {
            Pool::RaydiumAmmV4(p) => p.create_swap_instruction(token_in_mint, amount_in, min_amount_out, user_accounts),
            Pool::RaydiumCpmm(p) => p.create_swap_instruction(token_in_mint, amount_in, min_amount_out, user_accounts),
            Pool::RaydiumClmm(p) => p.create_swap_instruction(token_in_mint, amount_in, min_amount_out, user_accounts),
            Pool::MeteoraDammV1(p) => p.create_swap_instruction(token_in_mint, amount_in, min_amount_out, user_accounts),
            Pool::MeteoraDammV2(p) => p.create_swap_instruction(token_in_mint, amount_in, min_amount_out, user_accounts),
            Pool::MeteoraDlmm(p) => p.create_swap_instruction(token_in_mint, amount_in, min_amount_out, user_accounts),
            Pool::OrcaWhirlpool(p) => p.create_swap_instruction(token_in_mint, amount_in, min_amount_out, user_accounts),
            Pool::PumpAmm(p) => p.create_swap_instruction(token_in_mint, amount_in, min_amount_out, user_accounts),
        }
    }
}