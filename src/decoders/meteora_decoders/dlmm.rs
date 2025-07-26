// src/decoders/meteora_decoders/dlmm.rs

use crate::decoders::pool_operations::PoolOperations; // On importe le contrat
use bytemuck::{from_bytes, Pod, Zeroable};
use solana_sdk::pubkey::Pubkey;
use anyhow::{bail, Result, anyhow};
use crate::math::dlmm_math;
use std::collections::BTreeMap;

// Le VRAI discriminator que nous avons trouvé.
const DLMM_LBPAIR_DISCRIMINATOR: [u8; 8] = [33, 11, 49, 98, 181, 101, 177, 13];

// --- CONSTANTES (tirées de l'IDL) ---
const MAX_BIN_PER_ARRAY: i32 = 70;
const BIN_ARRAY_SEED: &[u8] = b"bin_array";



// --- STRUCTURES PUBLIQUES ---
/// Renommée pour plus de clarté
#[derive(Debug, Clone)]
pub struct DecodedDlmmPool {
    pub address: Pubkey,
    pub program_id: Pubkey,
    pub mint_a: Pubkey,
    pub mint_b: Pubkey,
    pub vault_a: Pubkey,
    pub vault_b: Pubkey,
    pub active_bin_id: i32,
    pub bin_step: u16,
    pub base_fee_percent: f64,
    pub reserve_a: u64,
    pub reserve_b: u64,
    // ----------------------------
    // Ce champ contiendra les BinArrays chargés par le graph_engine
    pub hydrated_bin_arrays: Option<BTreeMap<i64, Vec<u8>>>,
}


#[derive(Debug, Clone)]
pub struct DecodedBin {
    pub amount_a: u64,
    pub amount_b: u64,
}

// --- STRUCTURES BRUTES (Traduction 1:1 de l'IDL) ---

#[repr(C, packed)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
struct StaticParameters {
    pub base_factor: u16,
    pub filter_period: u16,
    pub decay_period: u16,
    pub reduction_factor: u16,
    pub variable_fee_control: u32,
    pub max_volatility_accumulator: u32,
    pub min_bin_id: i32,
    pub max_bin_id: i32,
    pub protocol_share: u16,
    pub base_fee_power_factor: u8,
    pub padding: [u8; 5],
}

#[repr(C, packed)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
struct VariableParameters {
    pub volatility_accumulator: u32,
    pub volatility_reference: u32,
    pub index_reference: i32,
    pub padding: [u8; 4],
    pub last_update_timestamp: i64,
    pub padding1: [u8; 8],
}

#[repr(C, packed)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
struct ProtocolFee {
    pub amount_x: u64,
    pub amount_y: u64,
}

#[repr(C, packed)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
struct RewardInfo {
    pub mint: Pubkey,
    pub vault: Pubkey,
    pub funder: Pubkey,
    pub reward_duration: u64,
    pub reward_duration_end: u64,
    pub reward_rate: u128,
    pub last_update_time: u64,
    pub cumulative_seconds_with_empty_liquidity_reward: u64,
}

#[repr(C, packed)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
struct LbPairData {
    pub parameters: StaticParameters,
    pub v_parameters: VariableParameters,
    pub bump_seed: [u8; 1],
    pub bin_step_seed: [u8; 2],
    pub pair_type: u8,
    pub active_id: i32,
    pub bin_step: u16,
    pub status: u8,
    pub require_base_factor_seed: u8,
    pub base_factor_seed: [u8; 2],
    pub activation_type: u8,
    pub creator_pool_on_off_control: u8,
    pub token_x_mint: Pubkey,
    pub token_y_mint: Pubkey,
    pub reserve_x: Pubkey,
    pub reserve_y: Pubkey,
    pub protocol_fee: ProtocolFee,
    pub padding1: [u8; 32],
    pub reward_infos: [RewardInfo; 2],
    pub oracle: Pubkey,
    pub bin_array_bitmap: [u64; 16],
    pub last_updated_at: i64,
    pub padding2: [u8; 32],
    pub pre_activation_swap_address: Pubkey,
    pub base_key: Pubkey,
    pub activation_point: u64,
    pub pre_activation_duration: u64,
    pub padding3: [u8; 8],
    pub padding4: u64,
    pub creator: Pubkey,
    pub token_mint_x_program_flag: u8,
    pub token_mint_y_program_flag: u8,
    pub reserved: [u8; 22],
}

#[repr(C, packed)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
pub struct Bin {
    pub amount_x: u64,
    pub amount_y: u64,
    pub price: u128,
    pub liquidity_supply: u128,
    pub reward_per_token_stored: [u128; 2],
    pub fee_amount_x_per_token_stored: u128,
    pub fee_amount_y_per_token_stored: u128,
    pub amount_x_in: u128,
    pub amount_y_in: u128,
}

// --- FONCTIONS PUBLIQUES ---

/// Décode le compte principal LbPair.
pub fn decode_lb_pair(address: &Pubkey, data: &[u8], program_id: &Pubkey) -> Result<DecodedDlmmPool> {
    if data.get(..8) != Some(&DLMM_LBPAIR_DISCRIMINATOR) {
        bail!("Invalid discriminator.");
    }
    let data_slice = &data[8..];
    if data_slice.len() != std::mem::size_of::<LbPairData>() {
        bail!(
            "LbPair data length mismatch. Expected {}, got {}.",
            std::mem::size_of::<LbPairData>(),
            data_slice.len()
        );
    }

    let pool_struct: &LbPairData = from_bytes(data_slice);

    let base_fee_rate = (pool_struct.parameters.base_factor as u64) * (pool_struct.bin_step as u64) * 10;
    let calculated_base_fee_percent = base_fee_rate as f64 / 1_000_000.0;

    Ok(DecodedDlmmPool {
        address: *address,
        program_id: *program_id, // On initialise le program_id
        mint_a: pool_struct.token_x_mint,
        mint_b: pool_struct.token_y_mint,
        vault_a: pool_struct.reserve_x,
        vault_b: pool_struct.reserve_y,
        active_bin_id: pool_struct.active_id,
        bin_step: pool_struct.bin_step,
        base_fee_percent: calculated_base_fee_percent,
        reserve_a: 0,
        reserve_b: 0,
        hydrated_bin_arrays: None, // On initialise le champ à None
    })
}


/// Calcule l'adresse du compte BinArray contenant un bin_id.
pub fn get_bin_array_address(lb_pair: &Pubkey, bin_id: i32) -> Pubkey {
    // Le calcul de l'index doit gérer les nombres négatifs correctement.
    let bin_array_index = (bin_id as i64 / MAX_BIN_PER_ARRAY as i64) - if bin_id < 0 && bin_id % MAX_BIN_PER_ARRAY != 0 { 1 } else { 0 };

    let (pda, _) = Pubkey::find_program_address(
        &[
            BIN_ARRAY_SEED,
            &lb_pair.to_bytes(),
            &bin_array_index.to_le_bytes(),
        ],
        &crate::decoders::meteora_decoders::ID,
    );
    pda
}

/// Décode la liquidité d'un bin spécifique depuis les données d'un BinArray.
pub fn decode_bin_from_bin_array(bin_id: i32, bin_array_data: &[u8]) -> Result<DecodedBin> {
    const BINS_OFFSET: usize = 8 + 1 + 7 + 32;
    let bin_index_in_array = (bin_id % MAX_BIN_PER_ARRAY + MAX_BIN_PER_ARRAY) % MAX_BIN_PER_ARRAY;
    let bin_offset = BINS_OFFSET + (bin_index_in_array as usize) * std::mem::size_of::<Bin>();

    if bin_array_data.len() < bin_offset + std::mem::size_of::<Bin>() {
        bail!("BinArray data is too short.");
    }

    let bin_struct: &Bin = from_bytes(&bin_array_data[bin_offset..bin_offset + std::mem::size_of::<Bin>()]);

    Ok(DecodedBin {
        amount_a: bin_struct.amount_x,
        amount_b: bin_struct.amount_y,
    })
}


impl PoolOperations for DecodedDlmmPool {
    fn get_mints(&self) -> (Pubkey, Pubkey) { (self.mint_a, self.mint_b) }
    fn get_vaults(&self) -> (Pubkey, Pubkey) { (self.vault_a, self.vault_b) }

    fn get_quote(&self, token_in_mint: &Pubkey, amount_in: u64) -> Result<u64> {
        let bin_arrays = self.hydrated_bin_arrays.as_ref()
            .ok_or_else(|| anyhow!("DLMM pool is not hydrated with BinArrays"))?;

        get_dlmm_quote_with_bins(self, amount_in, token_in_mint, bin_arrays)
    }
}


/// Calcule le montant de sortie pour un swap DLMM, en gérant le passage de plusieurs bins.
fn get_dlmm_quote_with_bins(
    pool: &DecodedDlmmPool,
    amount_in: u64,
    token_in_mint: &Pubkey,
    bin_arrays: &BTreeMap<i64, Vec<u8>>
) -> Result<u64> {

    let mut amount_remaining = amount_in as u128;
    let mut total_amount_out: u128 = 0;
    let mut current_bin_id = pool.active_bin_id;
    let is_base_input = *token_in_mint == pool.mint_a;

    while amount_remaining > 0 {
        let bin_array_idx = (current_bin_id as i64 / MAX_BIN_PER_ARRAY as i64) - if current_bin_id < 0 && current_bin_id % MAX_BIN_PER_ARRAY != 0 { 1 } else { 0 };
        let bin_array_data = bin_arrays.get(&bin_array_idx)
            .ok_or_else(|| anyhow!("Required BinArray at index {} not found for bin {}", bin_array_idx, current_bin_id))?;
        let current_bin = decode_bin_from_bin_array(current_bin_id, bin_array_data)?;

        let in_reserve = if is_base_input { current_bin.amount_a as u128 } else { current_bin.amount_b as u128 };

        if in_reserve == 0 {
            current_bin_id = if is_base_input { current_bin_id - 1 } else { current_bin_id + 1 };
            continue;
        }

        let amount_to_process = amount_remaining.min(in_reserve);
        let active_bin_price = dlmm_math::get_price_of_bin(current_bin_id, pool.bin_step)?;

        let amount_out_chunk = dlmm_math::get_amount_out(
            amount_to_process as u64,
            in_reserve,
            active_bin_price,
            is_base_input,
        )?;

        total_amount_out += amount_out_chunk as u128;
        amount_remaining -= amount_to_process;

        if amount_remaining > 0 {
            current_bin_id = if is_base_input { current_bin_id - 1 } else { current_bin_id + 1 };
        }
    }

    let fee = (total_amount_out as f64 * pool.base_fee_percent) as u128;
    Ok(total_amount_out.saturating_sub(fee) as u64)
}