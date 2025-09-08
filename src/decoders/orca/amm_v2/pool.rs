use crate::decoders::spl_token_decoders;
use anyhow::{anyhow, bail, Result};
use bytemuck::{from_bytes, Pod, Zeroable};
use serde::{Serialize, Deserialize};
use async_trait::async_trait;
use crate::decoders::pool_operations::{PoolOperations, UserSwapAccounts};
use solana_sdk::instruction::{AccountMeta, Instruction};
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;
use crate::rpc::ResilientRpcClient;



// ... (La struct DecodedOrcaAmmPool et son impl fee_as_percent ne changent pas)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecodedOrcaAmmPool {
    pub address: Pubkey,
    pub mint_a: Pubkey,
    pub mint_b: Pubkey,
    pub vault_a: Pubkey,
    pub vault_b: Pubkey,
    pub trade_fee_numerator: u64,
    pub trade_fee_denominator: u64,
    pub reserve_a: u64,
    pub reserve_b: u64,
    pub mint_a_transfer_fee_bps: u16,
    pub mint_b_transfer_fee_bps: u16,
    pub mint_a_decimals: u8,
    pub mint_b_decimals: u8,
    pub curve_type: u8,
    pub last_swap_timestamp: i64,
}

impl DecodedOrcaAmmPool {
    pub fn fee_as_percent(&self) -> f64 {
        if self.trade_fee_denominator == 0 {
            0.0
        } else {
            // On fait la multiplication par 100.0 en premier pour préserver la précision lors de la division
            (self.trade_fee_numerator as f64 * 100.0) / self.trade_fee_denominator as f64
        }
    }
}


// ... (Le module onchain_layouts et ses structs ne changent pas)
mod onchain_layouts {
    use super::*;
    #[repr(C, packed)]
    #[derive(Clone, Copy, Pod, Zeroable, Debug)]
    pub struct FeesData {
        pub trade_fee_numerator: u64, pub trade_fee_denominator: u64,
        pub owner_trade_fee_numerator: u64, pub owner_trade_fee_denominator: u64,
        pub owner_withdraw_fee_numerator: u64, pub owner_withdraw_fee_denominator: u64,
        pub host_fee_numerator: u64, pub host_fee_denominator: u64,
    }
    #[repr(C, packed)]
    #[derive(Clone, Copy, Pod, Zeroable, Debug)]
    pub struct SwapCurveData {
        pub curve_type: u8, pub curve_parameters: [u8; 32],
    }
    #[repr(C, packed)]
    #[derive(Clone, Copy, Pod, Zeroable, Debug)]
    pub struct OrcaTokenSwapV2PoolData {
        pub is_initialized: u8, pub bump_seed: u8, pub token_program_id: Pubkey,
        pub token_a_vault: Pubkey, pub token_b_vault: Pubkey, pub pool_mint: Pubkey,
        pub token_a_mint: Pubkey, pub token_b_mint: Pubkey, pub pool_fee_account: Pubkey,
        pub fees: FeesData, pub swap_curve: SwapCurveData,
    }
}


// --- LA CORRECTION EST DANS CETTE FONCTION ---
pub fn decode_pool(address: &Pubkey, data: &[u8]) -> Result<DecodedOrcaAmmPool> {
    let struct_len = size_of::<onchain_layouts::OrcaTokenSwapV2PoolData>(); // = 323 bytes

    // On s'attend maintenant à ce que les données on-chain fassent la taille de notre struct + 1 byte de préfixe.
    if data.len() != struct_len + 1 { // 324 bytes
        bail!(
            "Invalid data length for Orca V2 Pool. Expected {} bytes, got {}",
            struct_len + 1,
            data.len()
        );
    }

    // On ignore le premier byte et on ne décode que le reste.
    let data_slice = &data[1..];
    let pool_struct: &onchain_layouts::OrcaTokenSwapV2PoolData = from_bytes(data_slice);

    if pool_struct.is_initialized == 0 {
        bail!("Pool is not initialized.");
    }

    Ok(DecodedOrcaAmmPool {
        address: *address,
        mint_a: pool_struct.token_a_mint,
        mint_b: pool_struct.token_b_mint,
        vault_a: pool_struct.token_a_vault,
        vault_b: pool_struct.token_b_vault,
        trade_fee_numerator: pool_struct.fees.trade_fee_numerator,
        trade_fee_denominator: pool_struct.fees.trade_fee_denominator,
        curve_type: pool_struct.swap_curve.curve_type,
        reserve_a: 0,
        reserve_b: 0,
        mint_a_transfer_fee_bps: 0,
        mint_b_transfer_fee_bps: 0,
        mint_a_decimals: 0,
        mint_b_decimals: 0,
        last_swap_timestamp: 0,
    })
}

// ... (la fonction `hydrate` et l'implémentation de `PoolOperations` ne changent pas)
pub async fn hydrate(pool: &mut DecodedOrcaAmmPool, rpc_client: &ResilientRpcClient) -> Result<()> {
    let vaults_to_fetch = [pool.vault_a, pool.vault_b];
    let (vault_accounts_res, mint_a_res, mint_b_res) = tokio::join!(
        rpc_client.get_multiple_accounts(&vaults_to_fetch),
        rpc_client.get_account_data(&pool.mint_a),
        rpc_client.get_account_data(&pool.mint_b)
    );

    let vault_accounts = vault_accounts_res?;
    let vault_a_account = vault_accounts[0].as_ref().ok_or_else(|| anyhow!("Orca Vault A not found for pool {}", pool.address))?;
    pool.reserve_a = u64::from_le_bytes(vault_a_account.data[64..72].try_into()?);
    let vault_b_account = vault_accounts[1].as_ref().ok_or_else(|| anyhow!("Orca Vault B not found for pool {}", pool.address))?;
    pool.reserve_b = u64::from_le_bytes(vault_b_account.data[64..72].try_into()?);

    let decoded_mint_a = spl_token_decoders::mint::decode_mint(&pool.mint_a, &mint_a_res?)?;
    pool.mint_a_transfer_fee_bps = decoded_mint_a.transfer_fee_basis_points;
    pool.mint_a_decimals = decoded_mint_a.decimals;

    let decoded_mint_b = spl_token_decoders::mint::decode_mint(&pool.mint_b, &mint_b_res?)?;
    pool.mint_b_transfer_fee_bps = decoded_mint_b.transfer_fee_basis_points;
    pool.mint_b_decimals = decoded_mint_b.decimals;

    Ok(())
}
#[async_trait]
impl PoolOperations for DecodedOrcaAmmPool {
    fn get_mints(&self) -> (Pubkey, Pubkey) { (self.mint_a, self.mint_b) }
    fn get_vaults(&self) -> (Pubkey, Pubkey) { (self.vault_a, self.vault_b) }
    fn get_reserves(&self) -> (u64, u64) { (self.reserve_a, self.reserve_b) }
    fn address(&self) -> Pubkey { self.address }
    fn get_quote(&self, token_in_mint: &Pubkey, amount_in: u64, _current_timestamp: i64) -> Result<u64> {
        if self.trade_fee_denominator == 0 || self.reserve_a == 0 || self.reserve_b == 0 { return Ok(0); }
        let (in_reserve, out_reserve, in_mint_fee_bps, out_mint_fee_bps) = if *token_in_mint == self.mint_a {
            (self.reserve_a, self.reserve_b, self.mint_a_transfer_fee_bps, self.mint_b_transfer_fee_bps)
        } else if *token_in_mint == self.mint_b {
            (self.reserve_b, self.reserve_a, self.mint_b_transfer_fee_bps, self.mint_a_transfer_fee_bps)
        } else {
            return Err(anyhow!("Input token mint {} does not belong to the pool {}", token_in_mint, self.address));
        };
        let amount_in_u128 = amount_in as u128;
        let fee_on_input = (amount_in_u128 * in_mint_fee_bps as u128) / 10000;
        let amount_in_net = amount_in_u128.saturating_sub(fee_on_input);
        match self.curve_type {
            0 => {
                let fee_paid = (amount_in_net * self.trade_fee_numerator as u128) / self.trade_fee_denominator as u128;
                let amount_in_with_fees = amount_in_net.saturating_sub(fee_paid);
                let numerator = amount_in_with_fees * out_reserve as u128;
                let denominator = (in_reserve as u128).saturating_add(amount_in_with_fees);
                if denominator == 0 { return Ok(0); }
                let gross_amount_out = numerator / denominator;
                let fee_on_output = (gross_amount_out * out_mint_fee_bps as u128) / 10000;
                let final_amount_out = gross_amount_out.saturating_sub(fee_on_output);
                Ok(final_amount_out as u64)
            }
            _ => Err(anyhow!("Curve type {} is not supported for Orca AMM.", self.curve_type)),
        }
    }

    fn get_required_input(
        &mut self,
        token_out_mint: &Pubkey,
        amount_out: u64,
        _current_timestamp: i64,
    ) -> Result<u64> {
        if amount_out == 0 { return Ok(0); }
        if self.curve_type != 0 {
            return Err(anyhow!("Unsupported curve type for get_required_input in Orca AMM V2."));
        }

        let (in_reserve, out_reserve, in_mint_fee_bps, out_mint_fee_bps) = if *token_out_mint == self.mint_b {
            (self.reserve_a, self.reserve_b, self.mint_a_transfer_fee_bps, self.mint_b_transfer_fee_bps)
        } else {
            (self.reserve_b, self.reserve_a, self.mint_b_transfer_fee_bps, self.mint_a_transfer_fee_bps)
        };

        if out_reserve == 0 || in_reserve == 0 { return Err(anyhow!("Pool has no liquidity.")); }

        const BPS_DENOMINATOR: u128 = 10000;
        let gross_amount_out = if out_mint_fee_bps > 0 {
            let num = (amount_out as u128).saturating_mul(BPS_DENOMINATOR);
            let den = BPS_DENOMINATOR.saturating_sub(out_mint_fee_bps as u128);
            num.div_ceil(den)
        } else {
            amount_out as u128
        };

        if gross_amount_out >= out_reserve as u128 {
            return Err(anyhow!("Cannot get required input, amount_out is too high."));
        }

        let numerator = gross_amount_out.saturating_mul(in_reserve as u128);
        let denominator = (out_reserve as u128).saturating_sub(gross_amount_out);
        let amount_in_with_fees = numerator.div_ceil(denominator);

        let amount_in_net = if self.trade_fee_numerator > 0 {
            let num = amount_in_with_fees.saturating_mul(self.trade_fee_denominator as u128);
            let den = (self.trade_fee_denominator as u128).saturating_sub(self.trade_fee_numerator as u128);
            num.div_ceil(den)
        } else {
            amount_in_with_fees
        };

        let required_amount_in = if in_mint_fee_bps > 0 {
            let num = amount_in_net.saturating_mul(BPS_DENOMINATOR);
            let den = BPS_DENOMINATOR.saturating_sub(in_mint_fee_bps as u128);
            num.div_ceil(den)
        } else {
            amount_in_net
        };

        Ok(required_amount_in as u64)
    }


    fn create_swap_instruction(
        &self,
        _token_in_mint: &Pubkey,
        amount_in: u64,
        min_amount_out: u64,
        user_accounts: &UserSwapAccounts,
    ) -> Result<Instruction> {
        // Discriminateur pour l'instruction `swap` (valeur de 1)
        let mut instruction_data = vec![1];
        instruction_data.extend_from_slice(&amount_in.to_le_bytes());
        instruction_data.extend_from_slice(&min_amount_out.to_le_bytes());

        // Le programme V2 utilise une adresse de programme différente pour trouver l'autorité
        let (pool_authority, _) = Pubkey::find_program_address(
            &[&self.address.to_bytes()],
            &Pubkey::from_str("DjVE6JNiYqPL2QXyCUUh8rNjHrbz9hXHNYt99MQ59qw1")?, // Programme Token Swap V2
        );

        let accounts = vec![
            AccountMeta::new_readonly(self.address, false),
            AccountMeta::new_readonly(pool_authority, false),
            AccountMeta::new_readonly(user_accounts.owner, true),
            AccountMeta::new(user_accounts.source, false),
            AccountMeta::new(user_accounts.destination, false),
            AccountMeta::new(self.vault_a, false),
            AccountMeta::new(self.vault_b, false),
            AccountMeta::new(Pubkey::from_str("3wVrtQZ4C4H4D2E2zwy622m5Kjw3z1h76hA6F2SCL2sE")?, false), // Pool Mint (statique pour ce pool)
            AccountMeta::new(Pubkey::from_str("54q2ctpQ35a93r5wsd4p5a7Yw5f2s4ZifbUTk2MCR2Gq")?, false), // Fee Account (statique pour ce pool)
            AccountMeta::new_readonly(spl_token::id(), false),
        ];

        Ok(Instruction {
            program_id: Pubkey::from_str("DjVE6JNiYqPL2QXyCUUh8rNjHrbz9hXHNYt99MQ59qw1")?, // Programme Token Swap V2
            accounts,
            data: instruction_data,
        })
    }
}