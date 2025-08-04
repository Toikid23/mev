// DANS : src/decoders/meteora_decoders/dlmm.rs
// VERSION FINALE CORRIGÉE

use crate::decoders::pool_operations::PoolOperations;
use crate::decoders::spl_token_decoders;
use crate::math::dlmm_math::{self, FEE_PRECISION};
use anyhow::{anyhow, bail, Result};
use bytemuck::{from_bytes, pod_read_unaligned, Pod, Zeroable};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::pubkey::Pubkey;
use std::collections::BTreeMap;
use std::mem;

// --- CONSTANTES ---
const MAX_BIN_PER_ARRAY: usize = 70;
const BIN_ARRAY_SEED: &[u8] = b"bin_array";

// --- STRUCTURES (inchangées) ---

#[derive(Debug, Clone)]
pub struct DecodedDlmmPool {
    pub address: Pubkey, pub program_id: Pubkey, pub mint_a: Pubkey, pub mint_b: Pubkey,
    pub vault_a: Pubkey, pub vault_b: Pubkey, pub active_bin_id: i32, pub bin_step: u16,
    pub base_fee_rate: u64, pub mint_a_decimals: u8, pub mint_b_decimals: u8,
    pub mint_a_transfer_fee_bps: u16, pub mint_b_transfer_fee_bps: u16,
    pub parameters: onchain_layouts::StaticParameters, pub v_parameters: onchain_layouts::VariableParameters,
    pub hydrated_bin_arrays: Option<BTreeMap<i64, DecodedBinArray>>,
}

#[derive(Debug, Clone, Copy)]
pub struct DecodedBin { pub amount_a: u64, pub amount_b: u64, pub price: u128 }

#[derive(Debug, Clone, Copy)]
pub struct DecodedBinArray { pub index: i64, pub bins: [DecodedBin; MAX_BIN_PER_ARRAY] }


impl DecodedDlmmPool {
    pub fn fee_as_percent(&self) -> f64 { (self.base_fee_rate as f64 / 100_000.0) * 100.0 }

    fn calculate_swap_quote(&self, amount_in: u64, swap_for_y: bool) -> Result<u64> {
        let bin_arrays = self.hydrated_bin_arrays.as_ref().ok_or_else(|| anyhow!("Pool not hydrated"))?;
        let mut amount_remaining = amount_in as u128;
        let mut total_amount_out: u128 = 0;
        let mut current_bin_id = self.active_bin_id;

        let mut temp_v_params = self.v_parameters;
        let current_timestamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs() as i64;

        //println!("[DEBUG] === DEBUT SWAP ===");
        //println!("[DEBUG] Initial State: active_id={}, v_params={:?}", self.active_bin_id, self.v_parameters);

        update_references(&mut temp_v_params, &self.parameters, self.active_bin_id, current_timestamp)?;

        while amount_remaining > 0 {
            //println!("\n[DEBUG] Loop Start: amount_remaining={}, current_bin_id={}", amount_remaining, current_bin_id);
            if current_bin_id < self.parameters.min_bin_id || current_bin_id > self.parameters.max_bin_id {
                //println!("[DEBUG] Stop: Bin ID out of pool range.");
                break;
            }

            let bin_array_idx = get_bin_array_index_from_bin_id(current_bin_id);
            let bin_array = match bin_arrays.get(&bin_array_idx) {
                Some(array) => array,
                None => {
                    //println!("[DEBUG] Stop: BinArray index {} not found in hydrated map.", bin_array_idx);
                    break;
                }
            };

            let bin_index_in_array = (current_bin_id % (MAX_BIN_PER_ARRAY as i32) + (MAX_BIN_PER_ARRAY as i32)) % (MAX_BIN_PER_ARRAY as i32);
            let current_bin = &bin_array.bins[bin_index_in_array as usize];
            let (in_reserve, _) = if swap_for_y { (current_bin.amount_a, current_bin.amount_b) } else { (current_bin.amount_b, current_bin.amount_a) };

            //println!("[DEBUG] Bin {}: reserve_a={}, reserve_b={}, price={}", current_bin_id, current_bin.amount_a, current_bin.amount_b, current_bin.price);

            if in_reserve == 0 {
                //println!("[DEBUG] Bin a une réserve de 0. Passage au suivant.");
                current_bin_id = if swap_for_y { current_bin_id.saturating_sub(1) } else { current_bin_id.saturating_add(1) };
                continue;
            }

            update_volatility_accumulator(&mut temp_v_params, &self.parameters, self.active_bin_id, current_bin_id)?;
            //println!("[DEBUG] Updated v_params: {:?}", temp_v_params);

            let total_fee_rate = get_total_fee(self.bin_step, &self.parameters, &temp_v_params)?;
            //println!("[DEBUG] Calculated total_fee_rate: {} (sur une base de {})", total_fee_rate, FEE_PRECISION);

            if total_fee_rate >= FEE_PRECISION {
                //println!("[DEBUG] ERREUR: total_fee_rate >= FEE_PRECISION. Arrêt du swap.");
                break;
            }

            let max_amount_in_for_bin_with_fees = compute_amount_in_with_fees(in_reserve as u128, total_fee_rate)?;
            //println!("[DEBUG] max_amount_in_for_bin_with_fees: {}", max_amount_in_for_bin_with_fees);

            let amount_to_process_with_fees = amount_remaining.min(max_amount_in_for_bin_with_fees);
            //println!("[DEBUG] amount_to_process_with_fees: {}", amount_to_process_with_fees);

            let fee = (amount_to_process_with_fees * total_fee_rate) / FEE_PRECISION;
            //println!("[DEBUG] fee: {}", fee);

            let net_amount_in = amount_to_process_with_fees.saturating_sub(fee);
            //println!("[DEBUG] net_amount_in: {}", net_amount_in);

            if net_amount_in == 0 {
                //println!("[DEBUG] net_amount_in est 0, le swap ne peut pas progresser.");
                break;
            }

            let amount_out_chunk = dlmm_math::get_amount_out(net_amount_in as u64, current_bin.price, swap_for_y)?;
            //println!("[DEBUG] amount_out_chunk: {}", amount_out_chunk);

            total_amount_out += amount_out_chunk as u128;
            amount_remaining -= amount_to_process_with_fees;

            //println!("[DEBUG] Loop End: total_amount_out={}, amount_remaining={}", total_amount_out, amount_remaining);

            if amount_to_process_with_fees < max_amount_in_for_bin_with_fees {
                //println!("[DEBUG] Stop: Swap partiel dans le bin, tout le montant a été traité.");
                break;
            }
            current_bin_id = if swap_for_y { current_bin_id.saturating_sub(1) } else { current_bin_id.saturating_add(1) };
        }

        //println!("[DEBUG] === FIN SWAP ===");
        //println!("[DEBUG] Final total_amount_out: {}", total_amount_out);
        Ok(total_amount_out as u64)
    }
}

// ... (le reste des fonctions `impl PoolOperations`, `decode_lb_pair`, `hydrate`, etc., sont identiques au code précédent)

impl PoolOperations for DecodedDlmmPool {
    fn get_mints(&self) -> (Pubkey, Pubkey) { (self.mint_a, self.mint_b) }
    fn get_vaults(&self) -> (Pubkey, Pubkey) { (self.vault_a, self.vault_b) }

    fn get_quote(&self, token_in_mint: &Pubkey, amount_in: u64) -> Result<u64> {
        let swap_for_y = *token_in_mint == self.mint_a;

        let (in_mint_fee_bps, out_mint_fee_bps) = if swap_for_y {
            (self.mint_a_transfer_fee_bps, self.mint_b_transfer_fee_bps)
        } else {
            (self.mint_b_transfer_fee_bps, self.mint_a_transfer_fee_bps)
        };
        let fee_on_input = (amount_in as u128 * in_mint_fee_bps as u128) / 10000;
        let amount_in_after_transfer_fee = amount_in.saturating_sub(fee_on_input as u64);

        let gross_amount_out = self.calculate_swap_quote(amount_in_after_transfer_fee, swap_for_y)?;

        let fee_on_output = (gross_amount_out as u128 * out_mint_fee_bps as u128) / 10000;
        let final_amount_out = gross_amount_out.saturating_sub(fee_on_output as u64);

        Ok(final_amount_out)
    }
}
pub fn decode_lb_pair(address: &Pubkey, data: &[u8], program_id: &Pubkey) -> Result<DecodedDlmmPool> {
    const DISCRIMINATOR: [u8; 8] = [33, 11, 49, 98, 181, 101, 177, 13];
    if data.get(..8) != Some(&DISCRIMINATOR) {
        bail!("Invalid LbPair discriminator");
    }
    let data_slice = &data[8..];
    let pool_struct: &onchain_layouts::LbPairData = bytemuck::try_from_bytes(data_slice)
        .map_err(|e| anyhow!("LbPairData size mismatch or alignment error: {}", e))?;

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
        mint_a_decimals: 0,
        mint_b_decimals: 0,
        mint_a_transfer_fee_bps: 0,
        mint_b_transfer_fee_bps: 0,
        parameters: pool_struct.parameters,
        v_parameters: pool_struct.v_parameters,
        hydrated_bin_arrays: None,
    })
}

pub async fn hydrate(pool: &mut DecodedDlmmPool, rpc_client: &RpcClient, bin_array_fetch_range: i32) -> Result<()> {
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

fn get_bin_array_index_from_bin_id(bin_id: i32) -> i64 {
    (bin_id as i64 / (MAX_BIN_PER_ARRAY as i64))
        - if bin_id < 0 && bin_id % (MAX_BIN_PER_ARRAY as i32) != 0 { 1 } else { 0 }
}

fn get_bin_array_address(lb_pair: &Pubkey, bin_array_index: i64, program_id: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[BIN_ARRAY_SEED, &lb_pair.to_bytes(), &bin_array_index.to_le_bytes()], program_id).0
}

fn decode_bin_array(index: i64, data: &[u8]) -> Result<DecodedBinArray> {
    const DISCRIMINATOR: [u8; 8] = [92, 142, 92, 220, 5, 148, 70, 181];
    if data.get(..8) != Some(&DISCRIMINATOR) { bail!("Invalid BinArray discriminator"); }
    let data_slice = &data[8..];
    const BINS_FIELD_OFFSET: usize = 48;
    let mut bins = [DecodedBin { amount_a: 0, amount_b: 0, price: 0 }; MAX_BIN_PER_ARRAY];

    for i in 0..MAX_BIN_PER_ARRAY {
        let bin_offset = BINS_FIELD_OFFSET + (i * mem::size_of::<onchain_layouts::Bin>());
        let bin_end_offset = bin_offset + mem::size_of::<onchain_layouts::Bin>();
        if data_slice.len() < bin_end_offset { bail!("BinArray data slice too short to read bin #{}", i); }
        let bin_struct: onchain_layouts::Bin = pod_read_unaligned(&data_slice[bin_offset..bin_end_offset]);
        bins[i] = DecodedBin { amount_a: bin_struct.amount_x, amount_b: bin_struct.amount_y, price: bin_struct.price };
    }
    Ok(DecodedBinArray { index, bins })
}

// --- HELPERS DE CALCUL DE FRAIS (TRADUITS FIDÈLEMENT DU SDK) ---

fn get_base_fee(bin_step: u16, params: &onchain_layouts::StaticParameters) -> Result<u128> {
    Ok(u128::from(params.base_factor)
        .checked_mul(bin_step.into()).ok_or_else(|| anyhow!("MathOverflow"))?
        .checked_mul(10u128).ok_or_else(|| anyhow!("MathOverflow"))?
        .checked_mul(10u128.pow(params.base_fee_power_factor.into())).ok_or_else(|| anyhow!("MathOverflow"))?)
}

fn compute_variable_fee(volatility_accumulator: u32, bin_step: u16, params: &onchain_layouts::StaticParameters) -> Result<u128> {
    if params.variable_fee_control > 0 {
        let vfa: u128 = volatility_accumulator.into();
        let v_fee = vfa.checked_mul(bin_step.into()).ok_or_else(|| anyhow!("MathOverflow"))?
            .checked_pow(2).ok_or_else(|| anyhow!("MathOverflow"))?
            .checked_mul(params.variable_fee_control.into()).ok_or_else(|| anyhow!("MathOverflow"))?;

        return Ok(v_fee.checked_add(99_999_999_999).ok_or_else(|| anyhow!("MathOverflow"))?
            .checked_div(100_000_000_000).ok_or_else(|| anyhow!("MathOverflow"))?);
    }
    Ok(0)
}

fn get_total_fee(bin_step: u16, s_params: &onchain_layouts::StaticParameters, v_params: &onchain_layouts::VariableParameters) -> Result<u128> {
    let total_fee_rate = get_base_fee(bin_step, s_params)?
        .checked_add(compute_variable_fee(v_params.volatility_accumulator, bin_step, s_params)?).ok_or_else(|| anyhow!("MathOverflow"))?;
    const MAX_FEE_RATE: u128 = 100_000_000; // 10%
    Ok(total_fee_rate.min(MAX_FEE_RATE))
}

fn compute_amount_in_with_fees(amount_in: u128, total_fee_rate: u128) -> Result<u128> {
    let denominator = FEE_PRECISION.saturating_sub(total_fee_rate);
    if denominator == 0 { return Ok(u128::MAX); }
    amount_in.checked_mul(FEE_PRECISION).ok_or_else(|| anyhow!("MathOverflow"))?
        .checked_add(denominator - 1).ok_or_else(|| anyhow!("MathOverflow"))?
        .checked_div(denominator).ok_or_else(|| anyhow!("MathOverflow"))
}

fn update_references(v_params: &mut onchain_layouts::VariableParameters, s_params: &onchain_layouts::StaticParameters, active_id: i32, current_timestamp: i64) -> Result<()> {
    let elapsed = current_timestamp.checked_sub(v_params.last_update_timestamp).ok_or_else(|| anyhow!("MathOverflow: timestamp diff"))?;
    if elapsed >= s_params.filter_period as i64 {
        v_params.index_reference = active_id;
        if elapsed < s_params.decay_period as i64 {
            // La volatilité se dégrade en utilisant le facteur de réduction
            v_params.volatility_reference = v_params.volatility_accumulator
                .checked_mul(s_params.reduction_factor as u32).ok_or_else(|| anyhow!("MathOverflow"))?
                .checked_div(10000).ok_or_else(|| anyhow!("MathOverflow"))?;
        } else {
            // Après la période de dégradation, elle retombe à zéro
            v_params.volatility_reference = 0;
        }
    }
    Ok(())
}

fn update_volatility_accumulator(v_params: &mut onchain_layouts::VariableParameters, s_params: &onchain_layouts::StaticParameters, start_id: i32, end_id: i32) -> Result<()> {
    // La distance parcourue depuis la référence
    let delta_id = (i64::from(v_params.index_reference) - i64::from(end_id)).unsigned_abs();

    // Le nouvel accumulateur est la référence dégradée + la nouvelle distance parcourue
    let new_volatility_accumulator = u64::from(v_params.volatility_reference)
        .checked_add(delta_id.checked_mul(10000).ok_or_else(|| anyhow!("MathOverflow: delta_id mul"))?)
        .ok_or_else(|| anyhow!("MathOverflow: volatility_accumulator add"))?;

    // On plafonne par la valeur maximale autorisée et on met à jour
    v_params.volatility_accumulator = new_volatility_accumulator
        .min(s_params.max_volatility_accumulator as u64) as u32;

    Ok(())
}

// ... (le module onchain_layouts reste inchangé)
mod onchain_layouts {
    use super::*;

    #[repr(C, packed)]
    #[derive(Clone, Copy, Pod, Zeroable, Debug)]
    pub struct Bin {
        pub amount_x: u64, pub amount_y: u64, pub price: u128, pub liquidity_supply: u128,
        pub reward_per_token_stored: [u128; 2], pub fee_amount_x_per_token_stored: u128,
        pub fee_amount_y_per_token_stored: u128, pub amount_x_in: u128, pub amount_y_in: u128,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Pod, Zeroable, Debug)]
    pub struct StaticParameters {
        pub base_factor: u16, pub filter_period: u16, pub decay_period: u16,
        pub reduction_factor: u16, pub variable_fee_control: u32, pub max_volatility_accumulator: u32,
        pub min_bin_id: i32, pub max_bin_id: i32, pub protocol_share: u16,
        pub base_fee_power_factor: u8, pub padding: [u8; 5],
    }

    #[repr(C)]
    #[derive(Clone, Copy, Pod, Zeroable, Debug)]
    pub struct VariableParameters {
        pub volatility_accumulator: u32, pub volatility_reference: u32,
        pub index_reference: i32, pub padding: [u8; 4], pub last_update_timestamp: i64,
        pub padding1: [u8; 8],
    }

    #[repr(C, packed)]
    #[derive(Clone, Copy, Pod, Zeroable, Debug)]
    pub struct ProtocolFee { pub amount_x: u64, pub amount_y: u64 }

    #[repr(C, packed)]
    #[derive(Clone, Copy, Pod, Zeroable, Debug)]
    pub struct RewardInfo {
        pub mint: Pubkey, pub vault: Pubkey, pub funder: Pubkey, pub reward_duration: u64,
        pub reward_duration_end: u64, pub reward_rate: u128, pub last_update_time: u64,
        pub cumulative_seconds_with_empty_liquidity_reward: u64,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Pod, Zeroable, Debug)]
    pub struct LbPairData {
        pub parameters: StaticParameters, pub v_parameters: VariableParameters,
        pub bump_seed: [u8; 1], pub bin_step_seed: [u8; 2], pub pair_type: u8,
        pub active_id: i32, pub bin_step: u16, pub status: u8,
        pub require_base_factor_seed: u8, pub base_factor_seed: [u8; 2],
        pub activation_type: u8, pub creator_pool_on_off_control: u8,
        pub token_x_mint: Pubkey, pub token_y_mint: Pubkey, pub reserve_x: Pubkey,
        pub reserve_y: Pubkey, pub protocol_fee: ProtocolFee, pub padding1: [u8; 32],
        pub reward_infos: [RewardInfo; 2], pub oracle: Pubkey,
        pub bin_array_bitmap: [u64; 16], pub last_updated_at: i64, pub padding2: [u8; 32],
        pub pre_activation_swap_address: Pubkey, pub base_key: Pubkey,
        pub activation_point: u64, pub pre_activation_duration: u64,
        pub padding3: [u8; 8], pub padding4: u64, pub creator: Pubkey,
        pub token_mint_x_program_flag: u8, pub token_mint_y_program_flag: u8,
        pub reserved: [u8; 22],
    }
}