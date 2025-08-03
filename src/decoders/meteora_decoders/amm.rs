// DANS: src/decoders/meteora_decoders/amm.rs

use crate::decoders::pool_operations::PoolOperations;
use crate::math::meteora_stableswap_math;
use bytemuck::{pod_read_unaligned, Pod, Zeroable};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::program_pack::Pack;
use solana_sdk::pubkey::Pubkey;
use spl_token::state::{Account as SplTokenAccount, Mint as SplMint};
use anyhow::{anyhow, bail, Result};


// --- CONSTANTES ET STRUCTURES PUBLIQUES ---

pub const PROGRAM_ID: Pubkey = solana_sdk::pubkey!("Eo7WjKq67rjJQSZxS6z3YkapzY3eMj6Xy8X5EQVn5UaB");
const POOL_STATE_DISCRIMINATOR: [u8; 8] = [241, 154, 109, 4, 17, 177, 109, 188];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MeteoraCurveType {
    ConstantProduct,
    Stable {
        amp: u64,
        token_multiplier: onchain_layouts::TokenMultiplier,
    },
}

#[derive(Debug, Clone)]
pub struct DecodedMeteoraSbpPool {
    pub address: Pubkey,
    pub mint_a: Pubkey,
    pub mint_b: Pubkey,
    pub vault_a: Pubkey,
    pub vault_b: Pubkey,
    pub a_vault_lp: Pubkey,
    pub b_vault_lp: Pubkey,
    pub reserve_a: u64,
    pub reserve_b: u64,
    pub mint_a_decimals: u8,
    pub mint_b_decimals: u8,
    pub fees: onchain_layouts::PoolFees,
    pub curve_type: MeteoraCurveType,
    pub enabled: bool,
}

mod onchain_layouts {
    use super::*;
    #[repr(C, packed)]
    #[derive(Clone, Copy, Pod, Zeroable, Debug, PartialEq, Eq)]
    pub struct TokenMultiplier { pub token_a_multiplier: u64, pub token_b_multiplier: u64, pub precision_factor: u8 }
    #[repr(C, packed)]
    #[derive(Clone, Copy, Pod, Zeroable, Debug)]
    pub struct PoolFees { pub trade_fee_numerator: u64, pub trade_fee_denominator: u64, pub protocol_trade_fee_numerator: u64, pub protocol_trade_fee_denominator: u64 }
}

fn read_pod<T: Pod>(data: &[u8], offset: usize) -> Result<T> {
    let size = std::mem::size_of::<T>();
    let end = offset.checked_add(size).ok_or_else(|| anyhow!("Offset overflow"))?;
    if end > data.len() { bail!("Buffer underflow reading at offset {}", offset); }
    Ok(pod_read_unaligned(&data[offset..end]))
}

pub fn decode_pool(address: &Pubkey, data: &[u8]) -> Result<DecodedMeteoraSbpPool> {
    if data.get(..8) != Some(&POOL_STATE_DISCRIMINATOR) { bail!("Invalid discriminator."); }
    let data_slice = &data[8..];
    let token_a_mint: Pubkey = read_pod(data_slice, 32)?;
    let token_b_mint: Pubkey = read_pod(data_slice, 64)?;
    let a_vault: Pubkey = read_pod(data_slice, 96)?;
    let b_vault: Pubkey = read_pod(data_slice, 128)?;
    let a_vault_lp: Pubkey = read_pod(data_slice, 160)?;
    let b_vault_lp: Pubkey = read_pod(data_slice, 192)?;
    let enabled: u8 = read_pod(data_slice, 225)?;
    const FEES_OFFSET: usize = 322;
    let fees: onchain_layouts::PoolFees = read_pod(data_slice, FEES_OFFSET)?;
    const CURVE_TYPE_OFFSET: usize = 866;
    let curve_kind: u8 = read_pod(data_slice, CURVE_TYPE_OFFSET)?;

    let curve_type = match curve_kind {
        0 => MeteoraCurveType::ConstantProduct,
        1 => {
            const STABLE_PARAMS_OFFSET: usize = CURVE_TYPE_OFFSET + 1;
            let amp: u64 = read_pod(data_slice, STABLE_PARAMS_OFFSET)?;
            let token_multiplier: onchain_layouts::TokenMultiplier = read_pod(data_slice, STABLE_PARAMS_OFFSET + 8)?;
            MeteoraCurveType::Stable { amp, token_multiplier }
        }
        _ => bail!("Unsupported curve type: {}", curve_kind),
    };

    Ok(DecodedMeteoraSbpPool {
        address: *address,
        mint_a: token_a_mint, mint_b: token_b_mint,
        vault_a: a_vault, vault_b: b_vault,
        a_vault_lp, b_vault_lp,
        fees, curve_type, enabled: enabled == 1,
        reserve_a: 0, reserve_b: 0, mint_a_decimals: 0, mint_b_decimals: 0,
    })
}

// --- HYDRATE FINAL ET CORRECT ---
pub async fn hydrate(pool: &mut DecodedMeteoraSbpPool, rpc_client: &RpcClient) -> Result<()> {
    // Étape 1 : Récupérer tous les comptes nécessaires en parallèle
    let (vault_a_state_res, vault_b_state_res, pool_lp_a_res, pool_lp_b_res, mint_a_res, mint_b_res) = tokio::join!(
        rpc_client.get_account_data(&pool.vault_a),
        rpc_client.get_account_data(&pool.vault_b),
        rpc_client.get_account_data(&pool.a_vault_lp),
        rpc_client.get_account_data(&pool.b_vault_lp),
        rpc_client.get_account_data(&pool.mint_a),
        rpc_client.get_account_data(&pool.mint_b)
    );

    // Étape 2 : Décoder les décimales des tokens
    pool.mint_a_decimals = crate::decoders::spl_token_decoders::mint::decode_mint(&pool.mint_a, &mint_a_res?)?.decimals;
    pool.mint_b_decimals = crate::decoders::spl_token_decoders::mint::decode_mint(&pool.mint_b, &mint_b_res?)?.decimals;

    // Étape 3 : Lire la liquidité totale et les adresses des mints de LP depuis les comptes d'état des vaults
    let vault_a_state_data = vault_a_state_res?;
    let vault_b_state_data = vault_b_state_res?;

    // OFFSETS CORRECTS pour la structure `Vault` du programme `dynamic-vault`
    const TOTAL_AMOUNT_OFFSET: usize = 8 + 1 + 2; // discriminator + enabled + bumps = 11
    const LP_MINT_OFFSET: usize = TOTAL_AMOUNT_OFFSET + 8 + 32 + 32 + 32; // + total_amount + token_vault + fee_vault + token_mint = 115

    let total_amount_a: u64 = read_pod(&vault_a_state_data, TOTAL_AMOUNT_OFFSET)?;
    let vault_a_lp_mint_addr: Pubkey = read_pod(&vault_a_state_data, LP_MINT_OFFSET)?;

    let total_amount_b: u64 = read_pod(&vault_b_state_data, TOTAL_AMOUNT_OFFSET)?;
    let vault_b_lp_mint_addr: Pubkey = read_pod(&vault_b_state_data, LP_MINT_OFFSET)?;

    // Étape 4 : Récupérer les données des mints de LP
    let lp_mints_res = rpc_client.get_multiple_accounts(&[vault_a_lp_mint_addr, vault_b_lp_mint_addr]).await?;

    // Étape 5 : Désérialiser les comptes de parts et les mints de LP
    let pool_lp_a_account = SplTokenAccount::unpack(&pool_lp_a_res?)?;
    let pool_lp_b_account = SplTokenAccount::unpack(&pool_lp_b_res?)?;

    let vault_a_lp_mint = SplMint::unpack(&lp_mints_res[0].as_ref().ok_or_else(||anyhow!("Vault A LP mint not found"))?.data)?;
    let vault_b_lp_mint = SplMint::unpack(&lp_mints_res[1].as_ref().ok_or_else(||anyhow!("Vault B LP mint not found"))?.data)?;

    // Étape 6 : Calcul final et exact des réserves du pool
    if vault_a_lp_mint.supply > 0 {
        pool.reserve_a = u64::try_from(
            (total_amount_a as u128)
                .checked_mul(pool_lp_a_account.amount as u128).unwrap_or(0)
                .checked_div(vault_a_lp_mint.supply as u128).unwrap_or(0)
        )?;
    } else {
        pool.reserve_a = 0;
    }

    if vault_b_lp_mint.supply > 0 {
        pool.reserve_b = u64::try_from(
            (total_amount_b as u128)
                .checked_mul(pool_lp_b_account.amount as u128).unwrap_or(0)
                .checked_div(vault_b_lp_mint.supply as u128).unwrap_or(0)
        )?;
    } else {
        pool.reserve_b = 0;
    }

    Ok(())
}

impl PoolOperations for DecodedMeteoraSbpPool {
    fn get_mints(&self) -> (Pubkey, Pubkey) { (self.mint_a, self.mint_b) }
    fn get_vaults(&self) -> (Pubkey, Pubkey) { (self.vault_a, self.vault_b) }

    fn get_quote(&self, token_in_mint: &Pubkey, amount_in: u64) -> Result<u64> {
        if !self.enabled || amount_in == 0 { return Ok(0); }
        let (in_reserve, out_reserve) = if *token_in_mint == self.mint_a { (self.reserve_a, self.reserve_b) } else { (self.reserve_b, self.reserve_a) };
        if in_reserve == 0 || out_reserve == 0 { return Ok(0); }

        let gross_amount_out = match &self.curve_type {
            MeteoraCurveType::ConstantProduct => {
                let fee_numerator = self.fees.trade_fee_numerator;
                let fee_denominator = self.fees.trade_fee_denominator;
                if fee_denominator == 0 { return Ok(0); }

                let fee_amount = (amount_in as u128 * fee_numerator as u128) / fee_denominator as u128;
                let net_in = (amount_in as u128).saturating_sub(fee_amount);

                // Formule pure, car les réserves sont maintenant parfaites.
                let numerator = net_in * out_reserve as u128;
                let denominator = (in_reserve as u128).saturating_add(net_in);
                if denominator == 0 { return Ok(0); }

                (numerator / denominator) as u64
            }
            MeteoraCurveType::Stable { amp, token_multiplier } => {
                let total_fee_numerator = self.fees.trade_fee_numerator.saturating_add(self.fees.protocol_trade_fee_numerator);
                let fee_denominator = self.fees.trade_fee_denominator;
                if fee_denominator == 0 { return Ok(0); }

                let multiplier = *token_multiplier;
                let (norm_in_reserve, norm_out_reserve, in_mult, out_mult) = if *token_in_mint == self.mint_a {
                    (self.reserve_a as u128 * multiplier.token_a_multiplier as u128, self.reserve_b as u128 * multiplier.token_b_multiplier as u128, multiplier.token_a_multiplier, multiplier.token_b_multiplier)
                } else {
                    (self.reserve_b as u128 * multiplier.token_b_multiplier as u128, self.reserve_a as u128 * multiplier.token_a_multiplier as u128, multiplier.token_b_multiplier, multiplier.token_a_multiplier)
                };

                let norm_in = amount_in as u128 * in_mult as u128;
                let norm_out = meteora_stableswap_math::get_quote(norm_in as u64, norm_in_reserve as u64, norm_out_reserve as u64, *amp)? as u128;
                let amount_out = norm_out / out_mult as u128;
                let fee = (amount_out * total_fee_numerator as u128) / fee_denominator as u128;
                (amount_out.saturating_sub(fee)) as u64
            }
        };
        Ok(gross_amount_out)
    }
}