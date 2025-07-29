// DANS : src/decoders/meteora_decoders/dlmm.rs

use crate::decoders::pool_operations::PoolOperations;
use crate::decoders::spl_token_decoders;
use crate::math::dlmm_math;
use anyhow::{anyhow, bail, Result};
use bytemuck::{from_bytes, pod_read_unaligned, Zeroable, Pod};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::pubkey::Pubkey;
use std::collections::BTreeMap;
use std::mem;

// --- CONSTANTES DU PROTOCOLE ---
const MAX_BIN_PER_ARRAY: usize = 70;
const BIN_ARRAY_SEED: &[u8] = b"bin_array";

// --- STRUCTURES DE DONNÉES PUBLIQUES ET PROPRES ---

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
    pub base_fee_rate: u64,
    pub mint_a_transfer_fee_bps: u16,
    pub mint_b_transfer_fee_bps: u16,
    pub parameters: onchain_layouts::StaticParameters,
    pub v_parameters: onchain_layouts::VariableParameters,
    // Modèle de données optimisé : on stocke les BinArrays déjà décodés.
    pub hydrated_bin_arrays: Option<BTreeMap<i64, DecodedBinArray>>,
}

#[derive(Debug, Clone, Copy)]
pub struct DecodedBin {
    pub amount_a: u64,
    pub amount_b: u64,
    pub price: u128,
}

#[derive(Debug, Clone, Copy)]
pub struct DecodedBinArray {
    pub index: i64,
    // On stocke les bins décodés directement.
    pub bins: [DecodedBin; MAX_BIN_PER_ARRAY],
}

// --- LOGIQUE DU POOL ---

impl DecodedDlmmPool {
    pub fn fee_as_percent(&self) -> f64 {
        (self.base_fee_rate as f64 / 100_000.0) * 100.0
    }

    /// Calcule le swap en itérant à travers les bins pré-chargés.
    fn calculate_swap_across_bins(
        &self,
        amount_in: u64,
        swap_for_y: bool,
    ) -> Result<(u64, u64)> {
        let bin_arrays = self
            .hydrated_bin_arrays
            .as_ref()
            .ok_or_else(|| anyhow!("Pool not hydrated"))?;
        let mut amount_remaining = amount_in as u128;
        let mut total_amount_out: u128 = 0;
        let mut current_bin_id = self.active_bin_id;
        let mut bins_processed_count = 0u64;

        while amount_remaining > 0 {
            if current_bin_id < self.parameters.min_bin_id
                || current_bin_id > self.parameters.max_bin_id
            {
                break;
            }

            let bin_array_idx = get_bin_array_index_from_bin_id(current_bin_id);
            let bin_array = match bin_arrays.get(&bin_array_idx) {
                Some(array) => array,
                None => break, // Plus de liquidité dans notre fenêtre
            };

            let bin_index_in_array =
                (current_bin_id % (MAX_BIN_PER_ARRAY as i32) + (MAX_BIN_PER_ARRAY as i32))
                    % (MAX_BIN_PER_ARRAY as i32);
            let current_bin = &bin_array.bins[bin_index_in_array as usize];

            let (in_reserve, out_reserve) = if swap_for_y {
                (current_bin.amount_a, current_bin.amount_b)
            } else {
                (current_bin.amount_b, current_bin.amount_a)
            };

            if in_reserve == 0 {
                current_bin_id = if swap_for_y {
                    current_bin_id.saturating_sub(1)
                } else {
                    current_bin_id.saturating_add(1)
                };
                continue;
            }

            let amount_to_process = amount_remaining.min(in_reserve as u128);
            let amount_out_chunk =
                dlmm_math::get_amount_out(amount_to_process as u64, current_bin.price, swap_for_y)?;

            total_amount_out += amount_out_chunk as u128;
            amount_remaining -= amount_to_process;
            bins_processed_count += 1;

            // Si on n'a pas utilisé tout le bin, le swap s'arrête ici.
            if amount_to_process < in_reserve as u128 {
                break;
            }

            current_bin_id = if swap_for_y {
                current_bin_id.saturating_sub(1)
            } else {
                current_bin_id.saturating_add(1)
            };
        }

        let effective_bins_crossed = if bins_processed_count > 0 {
            bins_processed_count - 1
        } else {
            0
        };
        Ok((total_amount_out as u64, effective_bins_crossed))
    }
}

// --- CONTRAT POOLOPERATIONS ---

impl PoolOperations for DecodedDlmmPool {
    fn get_mints(&self) -> (Pubkey, Pubkey) {
        (self.mint_a, self.mint_b)
    }
    fn get_vaults(&self) -> (Pubkey, Pubkey) {
        (self.vault_a, self.vault_b)
    }

    fn get_quote(&self, token_in_mint: &Pubkey, amount_in: u64) -> Result<u64> {
        let swap_for_y = *token_in_mint == self.mint_a;

        let (in_mint_fee_bps, out_mint_fee_bps) = if swap_for_y {
            (self.mint_a_transfer_fee_bps, self.mint_b_transfer_fee_bps)
        } else {
            (self.mint_b_transfer_fee_bps, self.mint_a_transfer_fee_bps)
        };
        let fee_on_input = (amount_in as u128 * in_mint_fee_bps as u128) / 10000;
        let amount_in_after_transfer_fee = amount_in.saturating_sub(fee_on_input as u64);

        let (gross_amount_out, bins_crossed) =
            self.calculate_swap_across_bins(amount_in_after_transfer_fee, swap_for_y)?;

        let dynamic_fee_rate = dlmm_math::calculate_dynamic_fee(
            bins_crossed,
            self.v_parameters.volatility_accumulator,
            self.v_parameters.last_update_timestamp,
            self.bin_step,
            self.parameters.base_factor,
            self.parameters.filter_period,
            self.parameters.decay_period,
            self.parameters.reduction_factor,
            self.parameters.variable_fee_control,
            self.parameters.max_volatility_accumulator,
        )?;

        let total_fee_rate = self.base_fee_rate.saturating_add(dynamic_fee_rate);

        const FEE_PRECISION: u128 = 22_000_000_000_000;

        let total_fee_amount = (gross_amount_out as u128 * total_fee_rate as u128) / FEE_PRECISION;
        let net_amount_out = gross_amount_out.saturating_sub(total_fee_amount as u64);

        let fee_on_output = (net_amount_out as u128 * out_mint_fee_bps as u128) / 10000;
        let final_amount_out = net_amount_out.saturating_sub(fee_on_output as u64);

        Ok(final_amount_out)
    }
}

// --- FONCTIONS DE DÉCODAGE ET D'HYDRATATION ---

pub fn decode_lb_pair(
    address: &Pubkey,
    data: &[u8],
    program_id: &Pubkey,
) -> Result<DecodedDlmmPool> {
    const DISCRIMINATOR: [u8; 8] = [33, 11, 49, 98, 181, 101, 177, 13];
    if data.get(..8) != Some(&DISCRIMINATOR) {
        bail!("Invalid LbPair discriminator");
    }
    let data_slice = &data[8..];
    if data_slice.len() != mem::size_of::<onchain_layouts::LbPairData>() {
        bail!("LbPairData size mismatch");
    }
    let pool_struct: &onchain_layouts::LbPairData = from_bytes(data_slice);
    let base_fee_rate =
        (pool_struct.parameters.base_factor as u64).saturating_mul(pool_struct.bin_step as u64);

    Ok(DecodedDlmmPool {
        address: *address,
        program_id: *program_id,
        mint_a: pool_struct.token_x_mint,
        mint_b: pool_struct.token_y_mint,
        vault_a: pool_struct.reserve_x,
        vault_b: pool_struct.reserve_y,
        active_bin_id: pool_struct.active_id,
        bin_step: pool_struct.bin_step,
        base_fee_rate,
        mint_a_transfer_fee_bps: 0,
        mint_b_transfer_fee_bps: 0,
        parameters: pool_struct.parameters,
        v_parameters: pool_struct.v_parameters,
        hydrated_bin_arrays: None,
    })
}

pub async fn hydrate(
    pool: &mut DecodedDlmmPool,
    rpc_client: &RpcClient,
    bin_array_fetch_range: i32,
) -> Result<()> {
    let (mint_a_res, mint_b_res) = tokio::join!(
        rpc_client.get_account_data(&pool.mint_a),
        rpc_client.get_account_data(&pool.mint_b)
    );

    pool.mint_a_transfer_fee_bps =
        spl_token_decoders::mint::decode_mint(&pool.mint_a, &mint_a_res?)?
            .transfer_fee_basis_points;
    pool.mint_b_transfer_fee_bps =
        spl_token_decoders::mint::decode_mint(&pool.mint_b, &mint_b_res?)?
            .transfer_fee_basis_points;

    let active_array_idx = get_bin_array_index_from_bin_id(pool.active_bin_id);
    let mut addresses_to_fetch = Vec::new();
    let mut index_map = BTreeMap::new();

    for i in -bin_array_fetch_range..=bin_array_fetch_range {
        let array_index = active_array_idx + i as i64;
        let address = get_bin_array_address(&pool.address, array_index, &pool.program_id);
        addresses_to_fetch.push(address);
        index_map.insert(address, array_index);
    }

    let accounts = rpc_client.get_multiple_accounts(&addresses_to_fetch).await?;
    let mut hydrated_bin_arrays = BTreeMap::new();

    for (i, account) in accounts.into_iter().enumerate() {
        if let Some(acc) = account {
            let address = addresses_to_fetch[i];
            if let Some(idx) = index_map.get(&address) {
                if let Ok(decoded_array) = decode_bin_array(*idx, &acc.data) {
                    hydrated_bin_arrays.insert(*idx, decoded_array);
                }
            }
        }
    }

    pool.hydrated_bin_arrays = Some(hydrated_bin_arrays);
    Ok(())
}

// --- FONCTIONS HELPERS PRIVÉES ---

fn get_bin_array_index_from_bin_id(bin_id: i32) -> i64 {
    (bin_id as i64 / (MAX_BIN_PER_ARRAY as i64))
        - if bin_id < 0 && bin_id % (MAX_BIN_PER_ARRAY as i32) != 0 {
        1
    } else {
        0
    }
}

fn get_bin_array_address(lb_pair: &Pubkey, bin_array_index: i64, program_id: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[
            BIN_ARRAY_SEED,
            &lb_pair.to_bytes(),
            &bin_array_index.to_le_bytes(),
        ],
        program_id,
    )
        .0
}

/// Décode les données brutes d'un compte BinArray et retourne une structure propre.
fn decode_bin_array(index: i64, data: &[u8]) -> Result<DecodedBinArray> {
    const DISCRIMINATOR: [u8; 8] = [92, 142, 92, 220, 5, 148, 70, 181];
    if data.get(..8) != Some(&DISCRIMINATOR) {
        bail!("Invalid BinArray discriminator");
    }
    let data_slice = &data[8..];
    const BINS_FIELD_OFFSET: usize = 48;

    let mut bins = [DecodedBin {
        amount_a: 0,
        amount_b: 0,
        price: 0,
    }; MAX_BIN_PER_ARRAY];

    for i in 0..MAX_BIN_PER_ARRAY {
        let bin_offset = BINS_FIELD_OFFSET + (i * mem::size_of::<onchain_layouts::Bin>());
        let bin_end_offset = bin_offset + mem::size_of::<onchain_layouts::Bin>();
        if data_slice.len() < bin_end_offset {
            bail!("BinArray data slice too short");
        }
        let bin_struct: onchain_layouts::Bin =
            pod_read_unaligned(&data_slice[bin_offset..bin_end_offset]);
        bins[i] = DecodedBin {
            amount_a: bin_struct.amount_x,
            amount_b: bin_struct.amount_y,
            price: bin_struct.price,
        };
    }

    Ok(DecodedBinArray { index, bins })
}

// --- MODULE PRIVÉ POUR LES STRUCTURES ON-CHAIN ---
// Cache la complexité et nettoie l'espace de nom principal.
mod onchain_layouts {
    use super::*;

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

    #[repr(C)]
    #[derive(Clone, Copy, Pod, Zeroable, Debug)]
    pub struct StaticParameters {
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

    #[repr(C)]
    #[derive(Clone, Copy, Pod, Zeroable, Debug)]
    pub struct VariableParameters {
        pub volatility_accumulator: u32,
        pub volatility_reference: u32,
        pub index_reference: i32,
        pub padding: [u8; 4],
        pub last_update_timestamp: i64,
        pub padding1: [u8; 8],
    }

    #[repr(C, packed)]
    #[derive(Clone, Copy, Pod, Zeroable, Debug)]
    pub struct ProtocolFee {
        pub amount_x: u64,
        pub amount_y: u64,
    }

    #[repr(C, packed)]
    #[derive(Clone, Copy, Pod, Zeroable, Debug)]
    pub struct RewardInfo {
        pub mint: Pubkey,
        pub vault: Pubkey,
        pub funder: Pubkey,
        pub reward_duration: u64,
        pub reward_duration_end: u64,
        pub reward_rate: u128,
        pub last_update_time: u64,
        pub cumulative_seconds_with_empty_liquidity_reward: u64,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Pod, Zeroable, Debug)]
    pub struct LbPairData {
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
}