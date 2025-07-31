use crate::decoders::pool_operations::PoolOperations;
use crate::math::meteora_stableswap_math;
use bytemuck::{pod_read_unaligned, Pod, Zeroable};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::pubkey::Pubkey;
use anyhow::{anyhow, bail, Result};

// --- CONSTANTES ET STRUCTURES ---
pub const PROGRAM_ID: Pubkey = solana_sdk::pubkey!("Eo7WjKq67rjJQSZxS6z3YkapzY3eMj6Xy8X5EQVn5UaB");
const POOL_STATE_DISCRIMINATOR: [u8; 8] = [241, 154, 109, 4, 17, 177, 109, 188];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MeteoraCurveType {
    ConstantProduct,
    Stable {
        amp: u64,
        token_multiplier: TokenMultiplier,
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
    pub fees: PoolFees,
    pub curve_type: MeteoraCurveType,
    pub enabled: bool,
}

#[repr(C, packed)]
#[derive(Clone, Copy, Pod, Zeroable, Debug, PartialEq, Eq)]
pub struct TokenMultiplier {
    pub token_a_multiplier: u64,
    pub token_b_multiplier: u64,
    pub precision_factor: u8,
}

#[repr(C, packed)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
pub struct PoolFees {
    pub trade_fee_numerator: u64,
    pub trade_fee_denominator: u64,
    pub protocol_trade_fee_numerator: u64,
    pub protocol_trade_fee_denominator: u64,
}

fn read_pod<T: Pod>(data: &[u8], offset: usize) -> Result<T> {
    let size = std::mem::size_of::<T>();
    let end = offset.checked_add(size).ok_or_else(|| anyhow!("Offset overflow"))?;
    if end > data.len() { bail!("Buffer underflow"); }
    Ok(pod_read_unaligned(&data[offset..end]))
}

pub fn decode_pool(address: &Pubkey, data: &[u8]) -> Result<DecodedMeteoraSbpPool> {
    if data.get(..8) != Some(&POOL_STATE_DISCRIMINATOR) { bail!("Invalid discriminator."); }
    let data_slice = &data[8..];
    let token_a_mint: Pubkey = read_pod(data_slice, 32)?;
    let token_b_mint: Pubkey = read_pod(data_slice, 64)?;
    let a_vault: Pubkey = read_pod(data_slice, 96)?;
    let b_vault: Pubkey = read_pod(data_slice, 128)?;
    let fees: PoolFees = read_pod(data_slice, 322)?;
    let curve_type_u8: u8 = read_pod(data_slice, 866)?;

    let curve_type = match curve_type_u8 {
        0 => MeteoraCurveType::ConstantProduct,
        1 => {
            let amp: u64 = read_pod(data_slice, 867)?;
            let token_multiplier: TokenMultiplier = read_pod(data_slice, 875)?;
            MeteoraCurveType::Stable { amp, token_multiplier }
        }
        _ => bail!("Unsupported curve type"),
    };

    Ok(DecodedMeteoraSbpPool {
        address: *address,
        mint_a: token_a_mint, mint_b: token_b_mint,
        vault_a: a_vault, vault_b: b_vault,
        fees, curve_type, enabled: true,
        reserve_a: 0, reserve_b: 0,
        mint_a_decimals: 0, mint_b_decimals: 0,
    })
}

// --- HYDRATE (LOGIQUE SIMPLE ET CORRECTE) ---

/// Extrait la réserve (`totalAmount`) d'un compte d'état de Vault Meteora.
fn get_total_amount_from_vault_state(vault_state_data: &[u8]) -> Result<u64> {
    const TOTAL_AMOUNT_OFFSET: usize = 19;
    let end = TOTAL_AMOUNT_OFFSET + 8;
    if vault_state_data.len() < end { bail!("Données d'état du vault trop courtes"); }
    Ok(u64::from_le_bytes(vault_state_data[TOTAL_AMOUNT_OFFSET..end].try_into()?))
}

pub async fn hydrate(pool: &mut DecodedMeteoraSbpPool, rpc_client: &RpcClient) -> Result<()> {
    println!("[DEBUG HYDRATE] Lancement de l'hydratation...");

    let (vault_a_res, vault_b_res, mint_a_res, mint_b_res) = tokio::join!(
        rpc_client.get_account_data(&pool.vault_a),
        rpc_client.get_account_data(&pool.vault_b),
        rpc_client.get_account_data(&pool.mint_a),
        rpc_client.get_account_data(&pool.mint_b)
    );

    let vault_a_data = vault_a_res.map_err(|e| anyhow!("Impossible de fetch vault A: {}", e))?;
    pool.reserve_a = get_total_amount_from_vault_state(&vault_a_data)?;

    let vault_b_data = vault_b_res.map_err(|e| anyhow!("Impossible de fetch vault B: {}", e))?;
    pool.reserve_b = get_total_amount_from_vault_state(&vault_b_data)?;

    println!("[DEBUG HYDRATE] Réserves: A={}, B={}", pool.reserve_a, pool.reserve_b);

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
                // CORRECTION FINALE : On utilise UNIQUEMENT les frais de trading pour le calcul payé par l'utilisateur.
                let fee_num = self.fees.trade_fee_numerator;
                let fee_den = self.fees.trade_fee_denominator;

                if fee_den == 0 { return Ok(0); }
                let fee_amount = (amount_in as u128 * fee_num as u128) / fee_den as u128;
                let net_in = (amount_in as u128).saturating_sub(fee_amount);
                (net_in * out_reserve as u128) / (in_reserve as u128 + net_in)
            }
            MeteoraCurveType::Stable { amp, token_multiplier } => {
                // CORRECTION FINALE : On utilise UNIQUEMENT les frais de trading.
                let (norm_in_reserve, norm_out_reserve, in_mult, out_mult) = if *token_in_mint == self.mint_a {
                    (self.reserve_a as u128 * token_multiplier.token_a_multiplier as u128,
                     self.reserve_b as u128 * token_multiplier.token_b_multiplier as u128,
                     token_multiplier.token_a_multiplier,
                     token_multiplier.token_b_multiplier)
                } else {
                    (self.reserve_b as u128 * token_multiplier.token_b_multiplier as u128,
                     self.reserve_a as u128 * token_multiplier.token_a_multiplier as u128,
                     token_multiplier.token_b_multiplier,
                     token_multiplier.token_a_multiplier)
                };
                let norm_in = amount_in as u128 * in_mult as u128;
                let norm_out = meteora_stableswap_math::get_quote(norm_in as u64, norm_in_reserve as u64, norm_out_reserve as u64, *amp)? as u128;
                let amount_out = norm_out / out_mult as u128;
                let fee_num = self.fees.trade_fee_numerator;
                let fee_den = self.fees.trade_fee_denominator;
                let fee = (amount_out * fee_num as u128) / fee_den as u128;
                amount_out - fee
            }
        };
        Ok(gross_amount_out as u64)
    }
}