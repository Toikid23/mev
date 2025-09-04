// src/decoders/raydium_decoders/pool

use bytemuck::{from_bytes, Pod, Zeroable};
use solana_sdk::pubkey::Pubkey;
use anyhow::{bail, Result, anyhow};
use super::math;
use serde::{Serialize, Deserialize};
use async_trait::async_trait;
use solana_client::nonblocking::rpc_client::RpcClient;
use crate::decoders::pool_operations::{PoolOperations, UserSwapAccounts}; // Le trait et la struct
use solana_sdk::instruction::{AccountMeta, Instruction}; // Les types pour l'instruction
use spl_associated_token_account::get_associated_token_address; // Pour dériver un compte
use std::str::FromStr;


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecodedStableSwapPool {
    pub address: Pubkey,
    pub mint_a: Pubkey,
    pub mint_b: Pubkey,
    pub vault_a: Pubkey,
    pub vault_b: Pubkey,
    pub model_data_account: Pubkey,
    pub total_fee_percent: f64,
    // Le paramètre mathématique clé
    pub amp: u64,
    // Les réserves seront hydratées plus tard
    pub reserve_a: u64,
    pub reserve_b: u64,
    pub last_swap_timestamp: i64,
}


// --- STRUCTURES BRUTES (Traduction de stable_stats.rs) ---

#[repr(C, packed)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
pub struct Fees {
    pub min_separate_numerator: u64,
    pub min_separate_denominator: u64,
    pub trade_fee_numerator: u64,
    pub trade_fee_denominator: u64,
    pub pnl_numerator: u64,
    pub pnl_denominator: u64,
    pub swap_fee_numerator: u64,
    pub swap_fee_denominator: u64,
}

#[repr(C, packed)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
pub struct OutPutData {
    pub need_take_pnl_coin: u64, pub need_take_pnl_pc: u64,
    pub total_pnl_pc: u64, pub total_pnl_coin: u64,
    pub pool_open_time: u64, pub punish_pc_amount: u64,
    pub punish_coin_amount: u64, pub orderbook_to_init_time: u64,
    pub swap_coin_in_amount: u128, pub swap_pc_out_amount: u128,
    pub swap_pc_in_amount: u128, pub swap_coin_out_amount: u128,
    pub swap_pc_fee: u64, pub swap_coin_fee: u64,
}

#[repr(C, packed)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
pub struct StableSwapAmmInfoData {
    pub account_type: u64, pub status: u64, pub nonce: u64,
    pub order_num: u64, pub depth: u64, pub coin_decimals: u64,
    pub pc_decimals: u64, pub state: u64, pub reset_flag: u64,
    pub min_size: u64, pub vol_max_cut_ratio: u64, pub amount_wave: u64,
    pub coin_lot_size: u64, pub pc_lot_size: u64,
    pub min_price_multiplier: u64, pub max_price_multiplier: u64,
    pub sys_decimal_value: u64, pub abort_trade_factor: u64,
    pub price_tick_multiplier: u64, pub price_tick: u64,
    pub fees: Fees,
    pub out_put: OutPutData,
    pub coin_vault: Pubkey, pub pc_vault: Pubkey,
    pub coin_mint: Pubkey, pub pc_mint: Pubkey,
    pub lp_mint: Pubkey, pub model_data_key: Pubkey,
    pub open_orders: Pubkey, pub serum_market: Pubkey,
    pub serum_program: Pubkey, pub target_orders: Pubkey,
    pub amm_admin: Pubkey,
    pub padding: [u64; 64],
}

/// Décode le compte principal d'un pool Stable Swap.
/// Retourne une structure PARTIELLE qui doit être complétée par le ModelDataAccount.
pub fn decode_pool_info(_address: &Pubkey, data: &[u8]) -> Result<(StableSwapAmmInfoData, f64)> {
    if data.len() != std::mem::size_of::<StableSwapAmmInfoData>() {
        bail!("Stable Swap Pool data length mismatch.");
    }
    let pool_struct: &StableSwapAmmInfoData = from_bytes(data);
    let total_fee_percent = if pool_struct.fees.trade_fee_denominator == 0 {
        0.0
    } else {
        pool_struct.fees.trade_fee_numerator as f64 / pool_struct.fees.trade_fee_denominator as f64
    };
    Ok((*pool_struct, total_fee_percent))
}


/// Décode le ModelDataAccount pour en extraire le facteur d'amplification.
pub fn decode_model_data(data: &[u8]) -> Result<u64> {
    if data.len() < 8 {
        bail!("ModelDataAccount data is too short.");
    }
    let amp_bytes: [u8; 8] = data[0..8].try_into()?;
    Ok(u64::from_le_bytes(amp_bytes))
}

#[async_trait]
impl PoolOperations for DecodedStableSwapPool {
    fn get_mints(&self) -> (Pubkey, Pubkey) {
        (self.mint_a, self.mint_b)
    }

    fn get_vaults(&self) -> (Pubkey, Pubkey) {
        (self.vault_a, self.vault_b)
    }
    fn get_reserves(&self) -> (u64, u64) { (self.reserve_a, self.reserve_b) }
    fn address(&self) -> Pubkey { self.address }

    fn get_quote(&self, token_in_mint: &Pubkey, amount_in: u64, _current_timestamp: i64) -> Result<u64> {
        // Vérifier que le pool est hydraté (a des réserves et un amp)
        if self.amp == 0 || self.reserve_a == 0 || self.reserve_b == 0 {
            return Ok(0);
        }

        let (in_reserve, out_reserve) = if *token_in_mint == self.mint_a {
            (self.reserve_a, self.reserve_b)
        } else if *token_in_mint == self.mint_b {
            (self.reserve_b, self.reserve_a)
        } else {
            return Err(anyhow!("Input token does not belong to this pool."));
        };

        // Note: La logique de stableswap utilise des u128 pour la précision
        let amount_out = math::get_quote(
            amount_in as u128,
            in_reserve as u128,
            out_reserve as u128,
            self.amp
        )?;

        Ok(amount_out as u64)
    }

    fn get_required_input(
        &mut self,
        _token_out_mint: &Pubkey,
        _amount_out: u64,
        _current_timestamp: i64,
    ) -> Result<u64> {
        Err(anyhow!("get_required_input is not yet implemented for Raydium Stable Swap."))
    }

    fn create_swap_instruction(
        &self,
        token_in_mint: &Pubkey,
        amount_in: u64,
        min_amount_out: u64,
        user_accounts: &UserSwapAccounts,
    ) -> Result<Instruction> {
        // Discriminateur pour l'instruction `swap`
        let instruction_discriminator: [u8; 8] = [248, 198, 158, 145, 225, 117, 135, 200];
        let mut instruction_data = Vec::new();
        instruction_data.extend_from_slice(&instruction_discriminator);
        instruction_data.extend_from_slice(&amount_in.to_le_bytes());
        instruction_data.extend_from_slice(&min_amount_out.to_le_bytes());

        let (pool_authority, _) = Pubkey::find_program_address(&[b"amm_authority"], &Pubkey::from_str("SSwpkEEcbUqx4vtoEsgK9bGAoTVis3ZMrBPyGZ5eebT").unwrap());

        // Le compte de frais de l'admin est l'ATA du token de SORTIE, possédé par l'autorité du pool.
        let admin_fee_account = if *token_in_mint == self.mint_a {
            get_associated_token_address(&pool_authority, &self.mint_b)
        } else {
            get_associated_token_address(&pool_authority, &self.mint_a)
        };

        let accounts = vec![
            AccountMeta::new_readonly(spl_token::id(), false),
            AccountMeta::new(self.address, false),
            AccountMeta::new_readonly(pool_authority, false),
            AccountMeta::new(user_accounts.source, false),
            AccountMeta::new(user_accounts.destination, false),
            AccountMeta::new(self.vault_a, false),
            AccountMeta::new(self.vault_b, false),
            AccountMeta::new(admin_fee_account, false),
            AccountMeta::new_readonly(user_accounts.owner, true),
            AccountMeta::new_readonly(solana_sdk::sysvar::clock::ID, false),
        ];

        Ok(Instruction {
            program_id: Pubkey::from_str("SSwpkEEcbUqx4vtoEsgK9bGAoTVis3ZMrBPyGZ5eebT").unwrap(),
            accounts,
            data: instruction_data,
        })
    }
}