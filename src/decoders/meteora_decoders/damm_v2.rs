// DANS: src/decoders/meteora_decoders/damm_v2.rs
// VERSION FINALE, CORRIGÉE ET FIDÈLE AU SDK

use crate::decoders::pool_operations::PoolOperations;
use crate::decoders::spl_token_decoders;
use anyhow::{anyhow, bail, Result};
use bytemuck::{pod_read_unaligned, Pod, Zeroable};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::pubkey::Pubkey;
use uint::construct_uint;

construct_uint! { pub struct U256(4); }

// --- CONSTANTES ET STRUCTURES PUBLIQUES ---

pub const PROGRAM_ID: Pubkey = solana_sdk::pubkey!("cpamdpZCGKUy5JxQXB4dcpGPiikHawvSWAd6mEn1sGG");
pub const POOL_STATE_DISCRIMINATOR: [u8; 8] = [241, 154, 109, 4, 17, 177, 109, 188];

#[derive(Debug, Clone)]
pub struct DecodedMeteoraDammPool {
    pub address: Pubkey,
    pub mint_a: Pubkey,
    pub mint_b: Pubkey,
    pub vault_a: Pubkey,
    pub vault_b: Pubkey,
    pub liquidity: u128,
    pub sqrt_price: u128,
    pub sqrt_min_price: u128,
    pub sqrt_max_price: u128,
    pub collect_fee_mode: u8,
    pub mint_a_decimals: u8,
    pub mint_b_decimals: u8,
    pub mint_a_transfer_fee_bps: u16,
    pub mint_b_transfer_fee_bps: u16,
    pub pool_fees: onchain_layouts::PoolFeesStruct,
    pub activation_point: u64,
}

impl DecodedMeteoraDammPool {
    /// Retourne les frais de base en pourcentage.
    pub fn fee_as_percent(&self) -> f64 {
        let base_fee = self.pool_fees.base_fee.cliff_fee_numerator;
        if base_fee == 0 { return 0.0; }
        // Le `cliff_fee_numerator` est sur une base de 1_000_000_000 (FEE_DENOMINATOR)
        (base_fee as f64 / 1_000_000_000.0) * 100.0
    }
}

// --- MODULE PRIVÉ POUR LES STRUCTURES ON-CHAIN (MIROIR EXACT DU SDK) ---
pub mod onchain_layouts {
    #![allow(dead_code)]
    use super::*;

    #[repr(C, packed)] #[derive(Copy, Clone, Pod, Zeroable, Debug)]
    pub struct Pool {
        pub pool_fees: PoolFeesStruct, pub token_a_mint: Pubkey, pub token_b_mint: Pubkey, pub token_a_vault: Pubkey,
        pub token_b_vault: Pubkey, pub whitelisted_vault: Pubkey, pub partner: Pubkey, pub liquidity: u128,
        pub _padding: u128, pub protocol_a_fee: u64, pub protocol_b_fee: u64, pub partner_a_fee: u64,
        pub partner_b_fee: u64, pub sqrt_min_price: u128, pub sqrt_max_price: u128, pub sqrt_price: u128,
        pub activation_point: u64, pub activation_type: u8, pub pool_status: u8, pub token_a_flag: u8,
        pub token_b_flag: u8, pub collect_fee_mode: u8, pub pool_type: u8, pub _padding_0: [u8; 2],
        pub fee_a_per_liquidity: [u8; 32], pub fee_b_per_liquidity: [u8; 32], pub permanent_lock_liquidity: u128,
        pub metrics: PoolMetrics, pub creator: Pubkey, pub _padding_1: [u64; 6], pub reward_infos: [RewardInfo; 2],
    }

    #[repr(C, packed)] #[derive(Copy, Clone, Pod, Zeroable, Debug)]
    pub struct PoolFeesStruct {
        pub base_fee: BaseFeeStruct, pub protocol_fee_percent: u8, pub partner_fee_percent: u8,
        pub referral_fee_percent: u8, pub padding_0: [u8; 5], pub dynamic_fee: DynamicFeeStruct,
        pub padding_1: [u64; 2],
    }

    #[repr(C, packed)] #[derive(Copy, Clone, Pod, Zeroable, Debug)]
    pub struct BaseFeeStruct {
        pub cliff_fee_numerator: u64, pub fee_scheduler_mode: u8, pub padding_0: [u8; 5],
        pub number_of_period: u16, pub period_frequency: u64, pub reduction_factor: u64, pub padding_1: u64,
    }

    #[repr(C, packed)] #[derive(Copy, Clone, Pod, Zeroable, Debug)]
    pub struct DynamicFeeStruct {
        pub initialized: u8, pub padding: [u8; 7], pub max_volatility_accumulator: u32,
        pub variable_fee_control: u32, pub bin_step: u16, pub filter_period: u16, pub decay_period: u16,
        pub reduction_factor: u16, pub last_update_timestamp: u64, pub bin_step_u128: u128,
        pub sqrt_price_reference: u128, pub volatility_accumulator: u128, pub volatility_reference: u128,
    }

    #[repr(C, packed)] #[derive(Copy, Clone, Pod, Zeroable, Debug)]
    pub struct PoolMetrics {
        pub total_lp_a_fee: u128, pub total_lp_b_fee: u128, pub total_protocol_a_fee: u64,
        pub total_protocol_b_fee: u64, pub total_partner_a_fee: u64, pub total_partner_b_fee: u64,
        pub total_position: u64, pub padding: u64,
    }

    #[repr(C, packed)] #[derive(Copy, Clone, Pod, Zeroable, Debug)]
    pub struct RewardInfo {
        pub initialized: u8, pub reward_token_flag: u8, pub _padding_0: [u8; 6], pub _padding_1: [u8; 8],
        pub mint: Pubkey, pub vault: Pubkey, pub funder: Pubkey, pub reward_duration: u64,
        pub reward_duration_end: u64, pub reward_rate: u128, pub reward_per_token_stored: [u8; 32],
        pub last_update_time: u64, pub cumulative_seconds_with_empty_liquidity_reward: u64,
    }
}


// --- LOGIQUE DE DECODAGE ET D'HYDRATATION ---

pub fn decode_pool(address: &Pubkey, data: &[u8]) -> Result<DecodedMeteoraDammPool> {
    if data.get(..8) != Some(&POOL_STATE_DISCRIMINATOR) {
        bail!("Invalid discriminator. Not a Meteora DAMM v2 Pool account.");
    }
    let data_slice = &data[8..];
    let expected_size = std::mem::size_of::<onchain_layouts::Pool>();
    if data_slice.len() < expected_size {
        bail!(
            "DAMM v2 Pool data length mismatch. Expected at least {} bytes, got {}.",
            expected_size, data_slice.len()
        );
    }
    // "Caster" les données directement dans la structure miroir. Rapide et sûr.
    let pool_struct: onchain_layouts::Pool = bytemuck::pod_read_unaligned(&data_slice[..expected_size]);

    Ok(DecodedMeteoraDammPool {
        address: *address,
        mint_a: pool_struct.token_a_mint,
        mint_b: pool_struct.token_b_mint,
        vault_a: pool_struct.token_a_vault,
        vault_b: pool_struct.token_b_vault,
        liquidity: pool_struct.liquidity,
        sqrt_price: pool_struct.sqrt_price,
        sqrt_min_price: pool_struct.sqrt_min_price,
        sqrt_max_price: pool_struct.sqrt_max_price,
        collect_fee_mode: pool_struct.collect_fee_mode,
        pool_fees: pool_struct.pool_fees,
        activation_point: pool_struct.activation_point,
        mint_a_decimals: 0, mint_b_decimals: 0,
        mint_a_transfer_fee_bps: 0, mint_b_transfer_fee_bps: 0,
    })
}

pub async fn hydrate(pool: &mut DecodedMeteoraDammPool, rpc_client: &RpcClient) -> Result<()> {
    // La fonction `hydrate` est déjà correcte et efficace.
    let (mint_a_res, mint_b_res) = tokio::join!(
        rpc_client.get_account_data(&pool.mint_a),
        rpc_client.get_account_data(&pool.mint_b)
    );

    let mint_a_data = mint_a_res?;
    let decoded_mint_a = spl_token_decoders::mint::decode_mint(&pool.mint_a, &mint_a_data)?;
    pool.mint_a_decimals = decoded_mint_a.decimals;
    pool.mint_a_transfer_fee_bps = decoded_mint_a.transfer_fee_basis_points;

    let mint_b_data = mint_b_res?;
    let decoded_mint_b = spl_token_decoders::mint::decode_mint(&pool.mint_b, &mint_b_data)?;
    pool.mint_b_decimals = decoded_mint_b.decimals;
    pool.mint_b_transfer_fee_bps = decoded_mint_b.transfer_fee_basis_points;

    Ok(())
}

impl PoolOperations for DecodedMeteoraDammPool {
    fn get_mints(&self) -> (Pubkey, Pubkey) { (self.mint_a, self.mint_b) }
    fn get_vaults(&self) -> (Pubkey, Pubkey) { (self.vault_a, self.vault_b) }

    fn get_quote(&self, token_in_mint: &Pubkey, amount_in: u64, current_timestamp: i64) -> Result<u64> {
        if self.liquidity == 0 { return Ok(0); }

        let a_to_b = *token_in_mint == self.mint_a;

        let (in_mint_fee_bps, out_mint_fee_bps) = if a_to_b {
            (self.mint_a_transfer_fee_bps, self.mint_b_transfer_fee_bps)
        } else {
            (self.mint_b_transfer_fee_bps, self.mint_a_transfer_fee_bps)
        };

        // Étape 1 : Appliquer les frais de transfert Token-2022 (si applicable)
        let fee_on_input = (amount_in as u128 * in_mint_fee_bps as u128) / 10000;
        let amount_in_after_transfer_fee = amount_in.saturating_sub(fee_on_input as u64);

        let fees = &self.pool_fees;

        // Étape 2 : Calculer les frais de pool (base + dynamique)
        let base_fee_num = get_base_fee(&fees.base_fee, current_timestamp, self.activation_point)? as u128;
        let dynamic_fee_num = if fees.dynamic_fee.initialized != 0 {
            get_variable_fee(&fees.dynamic_fee)?
        } else { 0 };
        let total_fee_numerator = base_fee_num.saturating_add(dynamic_fee_num);
        const FEE_DENOMINATOR: u128 = 1_000_000_000;

        // Étape 3 : Déterminer si les frais s'appliquent sur l'input ou l'output
        let fees_on_input = match (self.collect_fee_mode, a_to_b) {
            (0, _) => false, // Collecte sur l'output
            (1, true) => false, // Collecte sur B (output)
            (1, false) => true, // Collecte sur B (input)
            _ => bail!("Unsupported collect_fee_mode"),
        };

        // Étape 4 : Calculer le swap
        let (pre_fee_amount_out, total_fee_amount) = if fees_on_input {
            let total_fee = (amount_in_after_transfer_fee as u128 * total_fee_numerator) / FEE_DENOMINATOR;
            let net_in = amount_in_after_transfer_fee.saturating_sub(total_fee as u64);

            let next_sqrt_price = get_next_sqrt_price_from_input(self.sqrt_price, self.liquidity, net_in, a_to_b)?;
            let out = get_amount_out(self.sqrt_price, next_sqrt_price, self.liquidity, a_to_b)?;
            (out, total_fee)
        } else {
            let next_sqrt_price = get_next_sqrt_price_from_input(self.sqrt_price, self.liquidity, amount_in_after_transfer_fee, a_to_b)?;
            let out = get_amount_out(self.sqrt_price, next_sqrt_price, self.liquidity, a_to_b)?;
            let total_fee = (out as u128 * total_fee_numerator) / FEE_DENOMINATOR;
            (out, total_fee)
        };

        // Étape 5 : Appliquer les frais LP (les seuls qui impactent l'utilisateur)
        let protocol_fee = (total_fee_amount * fees.protocol_fee_percent as u128) / 100;
        let lp_fee = total_fee_amount.saturating_sub(protocol_fee);

        let amount_out_after_pool_fee = if !fees_on_input {
            pre_fee_amount_out.saturating_sub(lp_fee as u64)
        } else {
            pre_fee_amount_out
        };

        // Étape 6 : Appliquer les frais de transfert Token-2022 sur l'output
        let fee_on_output = (amount_out_after_pool_fee as u128 * out_mint_fee_bps as u128) / 10000;
        let final_amount_out = amount_out_after_pool_fee.saturating_sub(fee_on_output as u64);

        Ok(final_amount_out)
    }
}


// --- FONCTIONS MATHÉMATIQUES (TRADUCTION DIRECTE DU SDK) ---

fn get_base_fee(base_fee: &onchain_layouts::BaseFeeStruct, current_timestamp: i64, activation_point: u64) -> Result<u64> {
    if base_fee.period_frequency == 0 {
        return Ok(base_fee.cliff_fee_numerator);
    }

    // Convertir en u64 pour les comparaisons et calculs.
    // Un timestamp négatif est un cas d'erreur que nous pouvons traiter comme étant "avant l'activation".
    let current_timestamp_u64 = u64::try_from(current_timestamp).unwrap_or(0);

    let period = if current_timestamp_u64 < activation_point {
        base_fee.number_of_period as u64
    } else {
        let elapsed = current_timestamp_u64.saturating_sub(activation_point);
        (elapsed / base_fee.period_frequency).min(base_fee.number_of_period as u64)
    };

    match base_fee.fee_scheduler_mode {
        0 => { // Mode Linéaire
            let reduction = period.saturating_mul(base_fee.reduction_factor);
            Ok(base_fee.cliff_fee_numerator.saturating_sub(reduction))
        }
        1 => { // Mode Exponentiel
            if base_fee.reduction_factor == 0 { return Ok(base_fee.cliff_fee_numerator); }
            let mut fee = base_fee.cliff_fee_numerator as u128;
            let reduction_factor = 10000 - base_fee.reduction_factor as u128;
            for _ in 0..period { fee = (fee * reduction_factor) / 10000; }
            Ok(fee as u64)
        }
        _ => bail!("Unsupported fee scheduler mode"),
    }
}

fn get_variable_fee(dynamic_fee: &onchain_layouts::DynamicFeeStruct) -> Result<u128> {
    let square_vfa_bin = dynamic_fee.volatility_accumulator
        .checked_mul(dynamic_fee.bin_step as u128).ok_or(anyhow!("MathOverflow"))?
        .checked_pow(2).ok_or(anyhow!("MathOverflow"))?;
    let v_fee = square_vfa_bin
        .checked_mul(dynamic_fee.variable_fee_control as u128).ok_or(anyhow!("MathOverflow"))?;
    let scaled_v_fee = v_fee
        .checked_add(99_999_999_999).ok_or(anyhow!("MathOverflow"))?
        .checked_div(100_000_000_000).ok_or(anyhow!("MathOverflow"))?;
    Ok(scaled_v_fee)
}

fn get_next_sqrt_price_from_input(sqrt_price: u128, liquidity: u128, amount_in: u64, a_for_b: bool) -> Result<u128> {
    if amount_in == 0 { return Ok(sqrt_price); }
    let sqrt_price_u256 = U256::from(sqrt_price);
    let liquidity_u256 = U256::from(liquidity);
    let amount_in_u256 = U256::from(amount_in);

    if a_for_b {
        let product = amount_in_u256.checked_mul(sqrt_price_u256).ok_or(anyhow!("MathOverflow"))?;
        let denominator = liquidity_u256.checked_add(product).ok_or(anyhow!("MathOverflow"))?;
        if denominator.is_zero() { return Err(anyhow!("Denominator is zero")); }
        let numerator = liquidity_u256.checked_mul(sqrt_price_u256).ok_or(anyhow!("MathOverflow"))?;
        let result = (numerator + denominator - U256::from(1)) / denominator;
        Ok(result.try_into().map_err(|_| anyhow!("TypeCastFailed"))?)
    } else {
        if liquidity_u256.is_zero() { return Err(anyhow!("Liquidity is zero")); }
        let quotient = (amount_in_u256 << 128) / liquidity_u256;
        let result = sqrt_price_u256.checked_add(quotient).ok_or(anyhow!("MathOverflow"))?;
        Ok(result.try_into().map_err(|_| anyhow!("TypeCastFailed"))?)
    }
}

fn get_amount_out(sqrt_price_start: u128, sqrt_price_end: u128, liquidity: u128, a_to_b: bool) -> Result<u64> {
    if a_to_b {
        get_delta_amount_b_unsigned(sqrt_price_end, sqrt_price_start, liquidity, Rounding::Down)
    } else {
        get_delta_amount_a_unsigned(sqrt_price_start, sqrt_price_end, liquidity, Rounding::Down)
    }
}

#[derive(PartialEq, Clone, Copy)] enum Rounding { Up, Down }

fn get_delta_amount_a_unsigned(lower_sqrt_price: u128, upper_sqrt_price: u128, liquidity: u128, round: Rounding) -> Result<u64> {
    let result = get_delta_amount_a_unsigned_unchecked(lower_sqrt_price, upper_sqrt_price, liquidity, round)?;
    if result > U256::from(u64::MAX) { bail!("MathOverflow"); }
    Ok(result.try_into().map_err(|_| anyhow!("TypeCastFailed"))?)
}

fn get_delta_amount_a_unsigned_unchecked(lower_sqrt_price: u128, upper_sqrt_price: u128, liquidity: u128, round: Rounding) -> Result<U256> {
    const RESOLUTION: u8 = 128;
    let num_1 = U256::from(liquidity) << RESOLUTION;
    let den_1 = U256::from(lower_sqrt_price);
    let den_2 = U256::from(upper_sqrt_price);
    if den_1.is_zero() || den_2.is_zero() { bail!("Sqrt price is zero"); }
    let term_1 = num_1 / den_1;
    let term_2 = num_1 / den_2;
    let diff = term_1 - term_2;
    let result = match round {
        Rounding::Up => (diff + (U256::from(1) << RESOLUTION) - U256::from(1)) >> RESOLUTION,
        Rounding::Down => diff >> RESOLUTION,
    };
    Ok(result)
}

fn get_delta_amount_b_unsigned(lower_sqrt_price: u128, upper_sqrt_price: u128, liquidity: u128, round: Rounding) -> Result<u64> {
    let result = get_delta_amount_b_unsigned_unchecked(lower_sqrt_price, upper_sqrt_price, liquidity, round)?;
    if result > U256::from(u64::MAX) { bail!("MathOverflow"); }
    Ok(result.try_into().map_err(|_| anyhow!("TypeCastFailed"))?)
}

fn get_delta_amount_b_unsigned_unchecked(lower_sqrt_price: u128, upper_sqrt_price: u128, liquidity: u128, round: Rounding) -> Result<U256> {
    const RESOLUTION: u8 = 64;
    let liquidity_u256 = U256::from(liquidity);
    let delta_sqrt_price = U256::from(upper_sqrt_price - lower_sqrt_price);
    let prod = liquidity_u256.checked_mul(delta_sqrt_price).ok_or(anyhow!("MathOverflow"))?;
    match round {
        Rounding::Up => {
            let denominator = U256::from(1) << (RESOLUTION as usize) * 2;
            Ok((prod + denominator - U256::from(1)) / denominator)
        }
        Rounding::Down => Ok(prod >> (RESOLUTION as usize) * 2),
    }
}