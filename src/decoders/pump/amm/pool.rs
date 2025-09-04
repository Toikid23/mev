// FICHIER : src/decoders/pump/amm/pool.rs

use bytemuck::{Pod, Zeroable};
use solana_sdk::pubkey::Pubkey;
use anyhow::{anyhow, bail, Result};
use solana_client::nonblocking::rpc_client::RpcClient;
use crate::decoders::spl_token_decoders;
use solana_sdk::instruction::{Instruction, AccountMeta};
use solana_sdk::system_program;
use spl_associated_token_account::get_associated_token_address_with_program_id;
use serde::{Serialize, Deserialize};
use async_trait::async_trait;
use crate::decoders::pool_operations::{PoolOperations, UserSwapAccounts};
use num_integer::Integer;
use std::str::FromStr;
use borsh::{BorshDeserialize};

pub const PUMP_PROGRAM_ID: Pubkey = solana_sdk::pubkey!("pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA");
pub const PUMP_FEE_PROGRAM_ID: Pubkey = solana_sdk::pubkey!("pfeeUxB6jkeY1Hxd7CsFCAjcbHA9rWtchMGdZ6VojVZ");
const POOL_ACCOUNT_DISCRIMINATOR: [u8; 8] = [241, 154, 109, 4, 17, 177, 109, 188];
const GLOBAL_CONFIG_ACCOUNT_DISCRIMINATOR: [u8; 8] = [149, 8, 156, 202, 160, 252, 176, 217];

const FEE_CONFIG_DISCRIMINATOR: [u8; 8] = [143, 52, 146, 187, 219, 123, 76, 155];


// --- STRUCTS POUR LES FRAIS DYNAMIQUES ---

#[derive(Debug, Clone, Serialize, Deserialize, Default, BorshDeserialize)]
pub struct DecodedFees {
    pub lp_fee_bps: u64,
    pub protocol_fee_bps: u64,
    pub creator_fee_bps: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, BorshDeserialize)]
pub struct DecodedFeeTier {
    pub market_cap_lamports_threshold: u128,
    pub fees: DecodedFees,
}

#[derive(Debug, Clone, Serialize, Deserialize, BorshDeserialize)]
pub struct DecodedFeeConfig {
    pub admin: Pubkey,
    pub flat_fees: DecodedFees,
    pub fee_tiers: Vec<DecodedFeeTier>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecodedPumpAmmPool {
    pub address: Pubkey,
    pub mint_a: Pubkey,
    pub mint_b: Pubkey,
    pub vault_a: Pubkey,
    pub vault_b: Pubkey,
    pub coin_creator: Pubkey,
    pub protocol_fee_recipients: [Pubkey; 8],
    pub reserve_a: u64,
    pub reserve_b: u64,
    pub mint_a_decimals: u8,
    pub mint_b_decimals: u8,
    pub mint_a_transfer_fee_bps: u16,
    pub mint_b_transfer_fee_bps: u16,
    pub lp_fee_basis_points: u64,
    pub protocol_fee_basis_points: u64,
    pub coin_creator_fee_basis_points: u64,
    pub mint_a_program: Pubkey,
    pub mint_b_program: Pubkey,
    pub last_swap_timestamp: i64,
}

pub mod onchain_layouts {
    use super::*;
    #[repr(C, packed)]
    #[derive(Clone, Copy, Pod, Zeroable, Debug)]
    pub struct Pool {
        pub pool_bump: u8,
        pub index: u16,
        pub creator: Pubkey,
        pub base_mint: Pubkey,
        pub quote_mint: Pubkey,
        pub lp_mint: Pubkey,
        pub pool_base_token_account: Pubkey,
        pub pool_quote_token_account: Pubkey,
        pub lp_supply: u64,
        pub coin_creator: Pubkey,
    }
    #[repr(C, packed)]
    #[derive(Clone, Copy, Pod, Zeroable, Debug)]
    pub struct GlobalConfig {
        pub admin: Pubkey,
        pub lp_fee_basis_points: u64,
        pub protocol_fee_basis_points: u64,
        pub disable_flags: u8,
        pub protocol_fee_recipients: [Pubkey; 8],
        pub coin_creator_fee_basis_points: u64,
        pub admin_set_coin_creator_authority: Pubkey,
    }
}

pub fn decode_pool(address: &Pubkey, data: &[u8]) -> Result<DecodedPumpAmmPool> {
    if data.get(..8) != Some(&POOL_ACCOUNT_DISCRIMINATOR) { bail!("Invalid discriminator."); }
    let data_slice = &data[8..];
    if data_slice.len() < std::mem::size_of::<onchain_layouts::Pool>() { bail!("Data length mismatch."); }
    let pool_struct: &onchain_layouts::Pool = bytemuck::from_bytes(&data_slice[..std::mem::size_of::<onchain_layouts::Pool>()]);
    Ok(DecodedPumpAmmPool {
        address: *address, mint_a: pool_struct.base_mint, mint_b: pool_struct.quote_mint,
        vault_a: pool_struct.pool_base_token_account, vault_b: pool_struct.pool_quote_token_account,
        coin_creator: pool_struct.coin_creator, protocol_fee_recipients: [Pubkey::default(); 8],
        reserve_a: 0, reserve_b: 0, mint_a_decimals: 0, mint_b_decimals: 0,
        mint_a_transfer_fee_bps: 0, mint_b_transfer_fee_bps: 0, lp_fee_basis_points: 0,
        protocol_fee_basis_points: 0, coin_creator_fee_basis_points: 0,
        mint_a_program: spl_token::id(), mint_b_program: spl_token::id(), last_swap_timestamp: 0,
    })
}

pub async fn hydrate(pool: &mut DecodedPumpAmmPool, rpc_client: &RpcClient) -> Result<()> {
    let (global_config_address, _) = Pubkey::find_program_address(&[b"global_config"], &PUMP_PROGRAM_ID);
    let accounts_to_fetch = vec![pool.vault_a, pool.vault_b, pool.mint_a, pool.mint_b, global_config_address];
    let accounts_data = rpc_client.get_multiple_accounts(&accounts_to_fetch).await?;

    let vault_a_data = accounts_data[0].as_ref().ok_or_else(|| anyhow!("Vault A not found"))?.data.clone();
    pool.reserve_a = u64::from_le_bytes(vault_a_data[64..72].try_into()?);

    let vault_b_data = accounts_data[1].as_ref().ok_or_else(|| anyhow!("Vault B not found"))?.data.clone();
    pool.reserve_b = u64::from_le_bytes(vault_b_data[64..72].try_into()?);

    let mint_a_account = accounts_data[2].as_ref().ok_or_else(|| anyhow!("Mint A not found"))?;
    let decoded_mint_a = spl_token_decoders::mint::decode_mint(&pool.mint_a, &mint_a_account.data)?;
    pool.mint_a_decimals = decoded_mint_a.decimals;
    pool.mint_a_transfer_fee_bps = decoded_mint_a.transfer_fee_basis_points;
    pool.mint_a_program = mint_a_account.owner;

    let mint_b_account = accounts_data[3].as_ref().ok_or_else(|| anyhow!("Mint B not found"))?;
    let decoded_mint_b = spl_token_decoders::mint::decode_mint(&pool.mint_b, &mint_b_account.data)?;
    pool.mint_b_decimals = decoded_mint_b.decimals;
    pool.mint_b_transfer_fee_bps = decoded_mint_b.transfer_fee_basis_points;
    pool.mint_b_program = mint_b_account.owner;

    let global_config_data = accounts_data[4].as_ref().ok_or_else(|| anyhow!("GlobalConfig not found"))?.data.clone();
    if global_config_data.get(..8) != Some(&GLOBAL_CONFIG_ACCOUNT_DISCRIMINATOR) { bail!("Invalid GlobalConfig discriminator"); }
    let config_data_slice = &global_config_data[8..];
    let config_struct: &onchain_layouts::GlobalConfig = bytemuck::from_bytes(&config_data_slice[..std::mem::size_of::<onchain_layouts::GlobalConfig>()]);
    pool.lp_fee_basis_points = config_struct.lp_fee_basis_points;
    pool.protocol_fee_basis_points = config_struct.protocol_fee_basis_points;
    pool.coin_creator_fee_basis_points = config_struct.coin_creator_fee_basis_points;
    pool.protocol_fee_recipients = config_struct.protocol_fee_recipients;
    Ok(())
}



#[async_trait]
impl PoolOperations for DecodedPumpAmmPool {
    fn get_mints(&self) -> (Pubkey, Pubkey) { (self.mint_a, self.mint_b) }
    fn get_vaults(&self) -> (Pubkey, Pubkey) { (self.vault_a, self.vault_b) }
    fn address(&self) -> Pubkey { self.address }

    fn get_quote(&self, token_in_mint: &Pubkey, amount_in: u64, _current_timestamp: i64) -> Result<u64> {
        if self.reserve_a == 0 || self.reserve_b == 0 { return Ok(0); }
        let is_buy = *token_in_mint == self.mint_b;

        let (in_reserve, out_reserve, in_mint_fee_bps, out_mint_fee_bps) = if is_buy {
            (self.reserve_b, self.reserve_a, self.mint_b_transfer_fee_bps, self.mint_a_transfer_fee_bps)
        } else {
            (self.reserve_a, self.reserve_b, self.mint_a_transfer_fee_bps, self.mint_b_transfer_fee_bps)
        };

        let fee_on_input = (amount_in as u128 * in_mint_fee_bps as u128) / 10_000;
        let amount_in_after_transfer_fee = amount_in.saturating_sub(fee_on_input as u64);
        let amount_in_u128 = amount_in_after_transfer_fee as u128;
        let in_reserve_u128 = in_reserve as u128;
        let out_reserve_u128 = out_reserve as u128;

        let total_fee_bps = if is_buy {
            100 // Pour un ACHAT (fixed-input), le total des frais est de 1%
        } else {
            let mut total = self.lp_fee_basis_points + self.protocol_fee_basis_points;
            if self.coin_creator != Pubkey::default() {
                total += self.coin_creator_fee_basis_points;
            }
            total
        };

        let amount_out_after_pool_fee = if is_buy {
            let denominator = 10_000u128.saturating_add(total_fee_bps as u128);
            let net_amount_in = amount_in_u128.saturating_mul(10_000).saturating_div(denominator);
            let numerator = out_reserve_u128.saturating_mul(net_amount_in);
            let effective_denominator = in_reserve_u128.saturating_add(net_amount_in);
            if effective_denominator == 0 { 0 } else { numerator / effective_denominator }
        } else {
            let numerator = amount_in_u128 * out_reserve_u128;
            let denominator = in_reserve_u128 + amount_in_u128;
            if denominator == 0 { return Ok(0); }
            let gross_amount_out = numerator / denominator;
            let total_fee = (gross_amount_out * total_fee_bps as u128) / 10_000;
            gross_amount_out.saturating_sub(total_fee)
        };

        let fee_on_output = (amount_out_after_pool_fee * out_mint_fee_bps as u128) / 10_000;
        Ok(amount_out_after_pool_fee.saturating_sub(fee_on_output) as u64)
    }

    fn get_required_input(&mut self, token_out_mint: &Pubkey, amount_out: u64, _current_timestamp: i64) -> Result<u64> {
        if amount_out == 0 { return Ok(0); }
        let is_buy = *token_out_mint == self.mint_a;

        let (in_reserve, out_reserve, in_mint_fee_bps, out_mint_fee_bps) = if is_buy {
            (self.reserve_b, self.reserve_a, self.mint_b_transfer_fee_bps, self.mint_a_transfer_fee_bps)
        } else {
            (self.reserve_a, self.reserve_b, self.mint_a_transfer_fee_bps, self.mint_b_transfer_fee_bps)
        };

        if out_reserve <= amount_out { return Err(anyhow!("Amount out is too high")); }
        const BPS_DENOMINATOR: u128 = 10_000;
        let gross_amount_out = if out_mint_fee_bps > 0 {
            (amount_out as u128).saturating_mul(BPS_DENOMINATOR).div_ceil(BPS_DENOMINATOR.saturating_sub(out_mint_fee_bps as u128))
        } else {
            amount_out as u128
        };
        if gross_amount_out >= out_reserve as u128 { return Err(anyhow!("Amount out too high")); }

        let required_input_before_transfer_fee = if is_buy {
            // 1. Inverser la formule AMM pour trouver le coût NET.
            let numerator = (in_reserve as u128).saturating_mul(gross_amount_out);
            let denominator = (out_reserve as u128).saturating_sub(gross_amount_out);
            let net_amount_in = numerator.div_ceil(denominator);

            // 2. Calculer les frais (LP + Protocole + Créateur) sur le coût NET.
            let lp_fee = (net_amount_in * self.lp_fee_basis_points as u128).div_ceil(BPS_DENOMINATOR);
            let protocol_fee = (net_amount_in * self.protocol_fee_basis_points as u128).div_ceil(BPS_DENOMINATOR);
            let creator_fee = if self.coin_creator != Pubkey::default() {
                (net_amount_in * self.coin_creator_fee_basis_points as u128).div_ceil(BPS_DENOMINATOR)
            } else {
                0
            };

            // 3. Retourner le coût BRUT (NET + tous les frais). C'est ce que le programme on-chain fait.
            net_amount_in.saturating_add(lp_fee).saturating_add(protocol_fee).saturating_add(creator_fee)

        } else {
            let mut total_fee_bps = self.lp_fee_basis_points + self.protocol_fee_basis_points;
            if self.coin_creator != Pubkey::default() { total_fee_bps += self.coin_creator_fee_basis_points; }
            let num_gross = gross_amount_out.saturating_mul(BPS_DENOMINATOR);
            let den_gross = BPS_DENOMINATOR.saturating_sub(total_fee_bps as u128);
            let gross_gross_out = num_gross.div_ceil(den_gross);
            if gross_gross_out >= out_reserve as u128 { return Err(anyhow!("Amount out too high")); }
            let numerator = (in_reserve as u128).saturating_mul(gross_gross_out);
            let denominator = (out_reserve as u128).saturating_sub(gross_gross_out);
            numerator.div_ceil(denominator)
        };

        let required_amount_in = if in_mint_fee_bps > 0 {
            required_input_before_transfer_fee.saturating_mul(BPS_DENOMINATOR).div_ceil(BPS_DENOMINATOR.saturating_sub(in_mint_fee_bps as u128))
        } else {
            required_input_before_transfer_fee
        };

        Ok(required_amount_in as u64)
    }

    async fn get_required_input_async(&mut self, token_out_mint: &Pubkey, amount_out: u64, _rpc_client: &RpcClient) -> Result<u64> { self.get_required_input(token_out_mint, amount_out, 0) }
    async fn get_quote_async(&mut self, token_in_mint: &Pubkey, amount_in: u64, _rpc_client: &RpcClient) -> Result<u64> { self.get_quote(token_in_mint, amount_in, 0) }

    fn create_swap_instruction(&self, token_in_mint: &Pubkey, amount_in: u64, min_amount_out: u64, user_accounts: &UserSwapAccounts) -> Result<Instruction> {
        let is_buy = *token_in_mint == self.mint_b;
        let mut instruction_data = Vec::new();
        if is_buy {
            let discriminator: [u8; 8] = [102, 6, 61, 18, 1, 218, 235, 234];
            instruction_data.extend_from_slice(&discriminator);
            instruction_data.extend_from_slice(&min_amount_out.to_le_bytes());
            instruction_data.extend_from_slice(&amount_in.to_le_bytes());
            instruction_data.push(0);
        } else {
            let discriminator: [u8; 8] = [51, 230, 133, 164, 1, 127, 131, 173];
            instruction_data.extend_from_slice(&discriminator);
            instruction_data.extend_from_slice(&amount_in.to_le_bytes());
            instruction_data.extend_from_slice(&min_amount_out.to_le_bytes());
        };
        let (user_base_token_account, user_quote_token_account) = if is_buy {
            (user_accounts.destination, user_accounts.source)
        } else {
            (user_accounts.source, user_accounts.destination)
        };
        let (global_config_address, _) = Pubkey::find_program_address(&[b"global_config"], &PUMP_PROGRAM_ID);
        let (fee_config, _) = Pubkey::find_program_address(&[b"fee_config", PUMP_PROGRAM_ID.as_ref()], &PUMP_FEE_PROGRAM_ID);
        let protocol_fee_recipient = self.protocol_fee_recipients.iter().find(|&&key| key != Pubkey::default()).unwrap_or(&self.protocol_fee_recipients[0]);
        let protocol_fee_recipient_token_account = get_associated_token_address_with_program_id(protocol_fee_recipient, &self.mint_b, &self.mint_b_program);
        let (coin_creator_vault_authority, _) = Pubkey::find_program_address(&[b"creator_vault", self.coin_creator.as_ref()], &PUMP_PROGRAM_ID);
        let coin_creator_vault_ata = get_associated_token_address_with_program_id(&coin_creator_vault_authority, &self.mint_b, &self.mint_b_program);
        let (user_volume_accumulator, _) = Pubkey::find_program_address(&[b"user_volume_accumulator", user_accounts.owner.as_ref()], &PUMP_PROGRAM_ID);
        let (global_volume_accumulator, _) = Pubkey::find_program_address(&[b"global_volume_accumulator"], &PUMP_PROGRAM_ID);
        let (event_authority, _) = Pubkey::find_program_address(&[b"__event_authority"], &PUMP_PROGRAM_ID);
        let accounts = vec![
            AccountMeta::new(self.address, false),
            AccountMeta::new(user_accounts.owner, true),
            AccountMeta::new_readonly(global_config_address, false),
            AccountMeta::new_readonly(self.mint_a, false),
            AccountMeta::new_readonly(self.mint_b, false),
            AccountMeta::new(user_base_token_account, false),
            AccountMeta::new(user_quote_token_account, false),
            AccountMeta::new(self.vault_a, false),
            AccountMeta::new(self.vault_b, false),
            AccountMeta::new_readonly(*protocol_fee_recipient, false),
            AccountMeta::new(protocol_fee_recipient_token_account, false),
            AccountMeta::new_readonly(self.mint_a_program, false),
            AccountMeta::new_readonly(self.mint_b_program, false),
            AccountMeta::new_readonly(solana_sdk::system_program::id(), false),
            AccountMeta::new_readonly(spl_associated_token_account::ID, false),
            AccountMeta::new_readonly(event_authority, false),
            AccountMeta::new_readonly(PUMP_PROGRAM_ID, false),
            AccountMeta::new(coin_creator_vault_ata, false),
            AccountMeta::new_readonly(coin_creator_vault_authority, false),
            AccountMeta::new(global_volume_accumulator, false),
            AccountMeta::new(user_volume_accumulator, false),
            AccountMeta::new_readonly(fee_config, false),
            AccountMeta::new_readonly(PUMP_FEE_PROGRAM_ID, false),
        ];
        Ok(Instruction {
            program_id: PUMP_PROGRAM_ID,
            accounts,
            data: instruction_data,
        })
    }
}