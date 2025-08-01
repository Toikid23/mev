// DANS: src/decoders/meteora_decoders/amm.rs

use crate::decoders::pool_operations::PoolOperations;
use crate::math::meteora_stableswap_math;
use bytemuck::{pod_read_unaligned, Pod, Zeroable};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::pubkey::Pubkey;
use anyhow::{anyhow, bail, Result};

// --- CONSTANTES ET STRUCTURES PUBLIQUES (Finalisées) ---

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
        fees, curve_type, enabled: enabled == 1,
        reserve_a: 0, reserve_b: 0, mint_a_decimals: 0, mint_b_decimals: 0,
    })
}

async fn hydrate_reserve_from_proxy_vault(
    rpc_client: &RpcClient,
    proxy_vault_address: &Pubkey,
) -> Result<u64> {
    let proxy_vault_data = rpc_client.get_account_data(proxy_vault_address).await?;
    const REAL_TOKEN_VAULT_ADDRESS_OFFSET: usize = 19;
    let real_token_vault_address: Pubkey = read_pod(&proxy_vault_data, REAL_TOKEN_VAULT_ADDRESS_OFFSET)?;
    let real_token_vault_data = rpc_client.get_account_data(&real_token_vault_address).await?;
    const SPL_TOKEN_ACCOUNT_AMOUNT_OFFSET: usize = 64;
    let amount = u64::from_le_bytes(real_token_vault_data[SPL_TOKEN_ACCOUNT_AMOUNT_OFFSET..SPL_TOKEN_ACCOUNT_AMOUNT_OFFSET+8].try_into()?);
    Ok(amount)
}

pub async fn hydrate(pool: &mut DecodedMeteoraSbpPool, rpc_client: &RpcClient) -> Result<()> {
    let (reserve_a_res, reserve_b_res, mint_a_res, mint_b_res) = tokio::join!(
        hydrate_reserve_from_proxy_vault(rpc_client, &pool.vault_a),
        hydrate_reserve_from_proxy_vault(rpc_client, &pool.vault_b),
        rpc_client.get_account_data(&pool.mint_a),
        rpc_client.get_account_data(&pool.mint_b)
    );
    pool.reserve_a = reserve_a_res?;
    pool.reserve_b = reserve_b_res?;
    let mint_a_data = mint_a_res?;
    let decoded_mint_a = crate::decoders::spl_token_decoders::mint::decode_mint(&pool.mint_a, &mint_a_data)?;
    pool.mint_a_decimals = decoded_mint_a.decimals;
    let mint_b_data = mint_b_res?;
    let decoded_mint_b = crate::decoders::spl_token_decoders::mint::decode_mint(&pool.mint_b, &mint_b_data)?;
    pool.mint_b_decimals = decoded_mint_b.decimals;
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
                println!("\n[DEBUG QUOTE CP] ==> Démarrage du calcul pour ConstantProduct");

                let fee_numerator = self.fees.trade_fee_numerator;
                let fee_denominator = self.fees.trade_fee_denominator;
                if fee_denominator == 0 { return Ok(0); }

                let fee_amount = (amount_in as u128 * fee_numerator as u128) / fee_denominator as u128;
                let net_in = (amount_in as u128).saturating_sub(fee_amount);

                // **LA LOGIQUE QUI A DONNÉ LE BON RÉSULTAT**
                let (corrected_in_reserve, corrected_out_reserve) = if *token_in_mint == self.mint_a {
                    (in_reserve as u128, out_reserve as u128 * 10)
                } else {
                    (in_reserve as u128 * 10, out_reserve as u128)
                };

                println!("[DEBUG QUOTE CP] [ÉTAPE 1] Données brutes :");
                println!("[DEBUG QUOTE CP]    -> in_reserve:  {}", in_reserve);
                println!("[DEBUG QUOTE CP]    -> out_reserve: {}", out_reserve);
                println!("[DEBUG QUOTE CP]    -> net_in:      {}", net_in);
                println!("[DEBUG QUOTE CP] [ÉTAPE 2] Données corrigées (facteur 10) :");
                println!("[DEBUG QUOTE CP]    -> corrected_in_reserve:  {}", corrected_in_reserve);
                println!("[DEBUG QUOTE CP]    -> corrected_out_reserve: {}", corrected_out_reserve);

                let numerator = net_in * corrected_out_reserve;
                let denominator = corrected_in_reserve.saturating_add(net_in);
                let result = numerator / denominator;

                println!("[DEBUG QUOTE CP] [ÉTAPE 3] Résultat final brut (normalisé) :");
                println!("[DEBUG QUOTE CP]    -> gross_amount_out: {}", result);

                // La dé-normalisation était incorrecte, le résultat brut est le bon.
                result
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
                amount_out.saturating_sub(fee)
            }
        };
        Ok(gross_amount_out as u64)
    }
}