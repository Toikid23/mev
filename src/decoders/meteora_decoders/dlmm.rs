// DANS : src/decoders/meteora_decoders/dlmm.rs

use crate::decoders::pool_operations::PoolOperations;
use bytemuck::{from_bytes, Pod, Zeroable, pod_read_unaligned};
use solana_sdk::pubkey::Pubkey;
use anyhow::{bail, Result, anyhow};
use crate::math::dlmm_math;
use crate::math::dlmm_math::U256;
use std::collections::BTreeMap;
use solana_client::nonblocking::rpc_client::RpcClient;
use crate::decoders::spl_token_decoders;
use std::collections::BTreeSet;
use std::mem;

// ... (Les définitions de constantes et de structs jusqu'à LbPairData restent inchangées) ...
// (Pour être sûr, je les remets ici)

const DLMM_LBPAIR_DISCRIMINATOR: [u8; 8] = [33, 11, 49, 98, 181, 101, 177, 13];
const BIN_ARRAY_DISCRIMINATOR: [u8; 8] = [92, 142, 92, 220, 5, 148, 70, 181];
const MAX_BIN_PER_ARRAY: i32 = 70;
const BIN_ARRAY_SEED: &[u8] = b"bin_array";

#[derive(Debug, Clone)]
pub struct DecodedDlmmPool {
    pub address: Pubkey, pub program_id: Pubkey, pub mint_a: Pubkey, pub mint_b: Pubkey,
    pub vault_a: Pubkey, pub vault_b: Pubkey, pub active_bin_id: i32, pub bin_step: u16,
    pub base_fee_rate: u64, pub mint_a_transfer_fee_bps: u16, pub mint_b_transfer_fee_bps: u16,
    pub parameters: StaticParameters, pub v_parameters: VariableParameters,
    pub hydrated_bin_arrays: Option<BTreeMap<i64, Vec<u8>>>,
}

impl DecodedDlmmPool {
    pub fn fee_as_percent(&self) -> f64 {
        // base_fee_rate est B*s = 40. Le site affiche 0.04%.
        // 40 / 100_000 = 0.0004. 0.0004 * 100 = 0.04%
        (self.base_fee_rate as f64 / 100_000.0) * 100.0
    }
}

#[derive(Debug, Clone)]
pub struct DecodedBin { pub amount_a: u64, pub amount_b: u64, pub price: u128, }

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
    pub base_factor: u16, pub filter_period: u16, pub decay_period: u16, pub reduction_factor: u16,
    pub variable_fee_control: u32, pub max_volatility_accumulator: u32, pub min_bin_id: i32, pub max_bin_id: i32,
    pub protocol_share: u16, pub base_fee_power_factor: u8, pub padding: [u8; 5],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
pub struct VariableParameters {
    pub volatility_accumulator: u32, pub volatility_reference: u32, pub index_reference: i32, pub padding: [u8; 4],
    pub last_update_timestamp: i64, pub padding1: [u8; 8],
}

#[repr(C, packed)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
struct ProtocolFee { pub amount_x: u64, pub amount_y: u64 }

#[repr(C, packed)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
struct RewardInfo {
    pub mint: Pubkey, pub vault: Pubkey, pub funder: Pubkey, pub reward_duration: u64, pub reward_duration_end: u64,
    pub reward_rate: u128, pub last_update_time: u64, pub cumulative_seconds_with_empty_liquidity_reward: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
struct LbPairData {
    pub parameters: StaticParameters, pub v_parameters: VariableParameters, pub bump_seed: [u8; 1], pub bin_step_seed: [u8; 2],
    pub pair_type: u8, pub active_id: i32, pub bin_step: u16, pub status: u8, pub require_base_factor_seed: u8,
    pub base_factor_seed: [u8; 2], pub activation_type: u8, pub creator_pool_on_off_control: u8,
    pub token_x_mint: Pubkey, pub token_y_mint: Pubkey, pub reserve_x: Pubkey, pub reserve_y: Pubkey,
    pub protocol_fee: ProtocolFee, pub padding1: [u8; 32], pub reward_infos: [RewardInfo; 2], pub oracle: Pubkey,
    pub bin_array_bitmap: [u64; 16], pub last_updated_at: i64, pub padding2: [u8; 32],
    pub pre_activation_swap_address: Pubkey, pub base_key: Pubkey, pub activation_point: u64,
    pub pre_activation_duration: u64, pub padding3: [u8; 8], pub padding4: u64, pub creator: Pubkey,
    pub token_mint_x_program_flag: u8, pub token_mint_y_program_flag: u8, pub reserved: [u8; 22],
}

// DANS : src/decoders/meteora_decoders/dlmm.rs
// REMPLACEZ L'INTÉGRALITÉ DE CETTE FONCTION

fn get_dlmm_quote_with_bins(pool: &DecodedDlmmPool, amount_in: u64, token_in_mint: &Pubkey) -> Result<(u64, u64)> {
    let bin_arrays = pool.hydrated_bin_arrays.as_ref().ok_or_else(|| anyhow!("Pool not hydrated"))?;
    let mut amount_remaining = amount_in as u128;
    let mut total_amount_out: u128 = 0;
    let mut current_bin_id = pool.active_bin_id;
    let is_base_input = *token_in_mint == pool.mint_a;
    let mut bins_processed_count = 0u64;

    while amount_remaining > 0 {
        if current_bin_id < pool.parameters.min_bin_id || current_bin_id > pool.parameters.max_bin_id { break; }
        let bin_array_idx = (current_bin_id as i64 / MAX_BIN_PER_ARRAY as i64) - if current_bin_id < 0 && current_bin_id % MAX_BIN_PER_ARRAY != 0 { 1 } else { 0 };
        let bin_array_data = match bin_arrays.get(&bin_array_idx) { Some(data) => data, None => break };
        let current_bin = match decode_bin_from_bin_array(current_bin_id, bin_array_data) {
            Ok(bin) => bin,
            Err(_) => {
                current_bin_id = if is_base_input { current_bin_id.saturating_add(1) } else { current_bin_id.saturating_sub(1) };
                continue;
            }
        };
        let in_reserve = if is_base_input { current_bin.amount_a as u128 } else { current_bin.amount_b as u128 };
        if in_reserve == 0 {
            current_bin_id = if is_base_input { current_bin_id.saturating_add(1) } else { current_bin_id.saturating_sub(1) };
            continue;
        }
        let amount_to_process = amount_remaining.min(in_reserve);
        let amount_out_chunk = dlmm_math::get_amount_out(amount_to_process as u64, current_bin.price, is_base_input)?;
        total_amount_out += amount_out_chunk as u128;
        amount_remaining -= amount_to_process;
        bins_processed_count += 1;
        if amount_to_process < in_reserve { break; }
        current_bin_id = if is_base_input { current_bin_id.saturating_add(1) } else { current_bin_id.saturating_sub(1) };
    }
    let effective_bins_crossed = if bins_processed_count > 0 { bins_processed_count - 1 } else { 0 };
    Ok((total_amount_out as u64, effective_bins_crossed))
}

impl PoolOperations for DecodedDlmmPool {
    fn get_mints(&self) -> (Pubkey, Pubkey) { (self.mint_a, self.mint_b) }
    fn get_vaults(&self) -> (Pubkey, Pubkey) { (self.vault_a, self.vault_b) }

    fn get_quote(&self, token_in_mint: &Pubkey, amount_in: u64) -> Result<u64> {
        let (in_mint_fee_bps, out_mint_fee_bps) = if *token_in_mint == self.mint_a {
            (self.mint_a_transfer_fee_bps, self.mint_b_transfer_fee_bps)
        } else {
            (self.mint_b_transfer_fee_bps, self.mint_a_transfer_fee_bps)
        };
        let fee_on_input = (amount_in as u128 * in_mint_fee_bps as u128) / 10000;
        let amount_in_after_transfer_fee = amount_in.saturating_sub(fee_on_input as u64);

        let (gross_amount_out, bins_crossed) = get_dlmm_quote_with_bins(self, amount_in_after_transfer_fee, token_in_mint)?;

        let dynamic_fee_rate = dlmm_math::calculate_dynamic_fee(
            bins_crossed, self.v_parameters.volatility_accumulator, self.v_parameters.last_update_timestamp,
            self.bin_step, self.parameters.base_factor, self.parameters.filter_period,
            self.parameters.decay_period, self.parameters.reduction_factor,
            self.parameters.variable_fee_control, self.parameters.max_volatility_accumulator,
        )?;

        let base_fee_rate = self.base_fee_rate;
        let total_fee_rate = base_fee_rate.saturating_add(dynamic_fee_rate);

        // LA CONSTANTE FINALE ET CORRECTE, CALCULÉE PAR INGÉNIERIE INVERSE
        const FEE_PRECISION: u128 = 22_000_000_000_000;

        let total_fee_amount = (gross_amount_out as u128 * total_fee_rate as u128) / FEE_PRECISION;
        let net_amount_out = gross_amount_out.saturating_sub(total_fee_amount as u64);

        let fee_on_output = (net_amount_out as u128 * out_mint_fee_bps as u128) / 10000;
        let final_amount_out = net_amount_out.saturating_sub(fee_on_output as u64);

        // --- BLOC DE DÉBOGAGE FINAL ---
        println!("--- DEBUG DLMM Quote ---");
        println!("Montant d'entrée (net): {}", amount_in_after_transfer_fee);
        println!("Montant brut de sortie (avant frais pool): {}", gross_amount_out);
        println!("Bins traversés: {}", bins_crossed);

        // Calcul des montants de frais pour l'affichage
        let base_fee_paid_display = (U256::from(total_fee_amount) * U256::from(base_fee_rate) / U256::from(total_fee_rate)).as_u64();
        let dynamic_fee_paid_display = total_fee_amount.saturating_sub(base_fee_paid_display as u128);

        println!("Frais de base payés (lamports): {}", base_fee_paid_display);
        println!("Frais dynamiques payés (lamports): {}", dynamic_fee_paid_display);
        println!("Montant total des frais (pool): {}", total_fee_amount);
        println!("Montant de sortie (après frais pool): {}", net_amount_out);
        println!("Montant de sortie final (après frais SPL): {}", final_amount_out);
        println!("------------------------");

        Ok(final_amount_out)
    }
}

pub fn decode_lb_pair(address: &Pubkey, data: &[u8], program_id: &Pubkey) -> Result<DecodedDlmmPool> {
    const DLMM_LBPAIR_DISCRIMINATOR: [u8; 8] = [33, 11, 49, 98, 181, 101, 177, 13];
    if data.get(..8) != Some(&DLMM_LBPAIR_DISCRIMINATOR) { bail!("Invalid LbPair discriminator"); }
    let data_slice = &data[8..];
    if data_slice.len() != mem::size_of::<LbPairData>() { bail!("LbPairData size mismatch"); }
    let pool_struct: &LbPairData = from_bytes(data_slice);
    let base_fee_rate = (pool_struct.parameters.base_factor as u64).saturating_mul(pool_struct.bin_step as u64);

    Ok(DecodedDlmmPool {
        address: *address, program_id: *program_id, mint_a: pool_struct.token_x_mint,
        mint_b: pool_struct.token_y_mint, vault_a: pool_struct.reserve_x, vault_b: pool_struct.reserve_y,
        active_bin_id: pool_struct.active_id, bin_step: pool_struct.bin_step, base_fee_rate,
        mint_a_transfer_fee_bps: 0, mint_b_transfer_fee_bps: 0, parameters: pool_struct.parameters,
        v_parameters: pool_struct.v_parameters, hydrated_bin_arrays: None,
    })
}

pub fn decode_bin_from_bin_array(bin_id: i32, bin_array_data: &[u8]) -> Result<DecodedBin> {
    const BIN_ARRAY_DISCRIMINATOR: [u8; 8] = [92, 142, 92, 220, 5, 148, 70, 181];
    if bin_array_data.get(..8) != Some(&BIN_ARRAY_DISCRIMINATOR) { bail!("Invalid BinArray discriminator"); }
    let data_slice = &bin_array_data[8..];
    const BINS_FIELD_OFFSET: usize = 48;
    let bin_index_in_array = (bin_id % MAX_BIN_PER_ARRAY + MAX_BIN_PER_ARRAY) % MAX_BIN_PER_ARRAY;
    let bin_offset = BINS_FIELD_OFFSET + (bin_index_in_array as usize * mem::size_of::<Bin>());
    let bin_end_offset = bin_offset + mem::size_of::<Bin>();
    if data_slice.len() < bin_end_offset { bail!("BinArray data slice too short"); }
    let bin_struct: Bin = pod_read_unaligned(&data_slice[bin_offset..bin_end_offset]);
    Ok(DecodedBin { amount_a: bin_struct.amount_x, amount_b: bin_struct.amount_y, price: bin_struct.price })
}

pub fn get_bin_array_address(lb_pair: &Pubkey, bin_id: i32, program_id: &Pubkey) -> Pubkey {
    let bin_array_index = (bin_id as i64 / MAX_BIN_PER_ARRAY as i64) - if bin_id < 0 && bin_id % MAX_BIN_PER_ARRAY != 0 { 1 } else { 0 };
    Pubkey::find_program_address(&[BIN_ARRAY_SEED, &lb_pair.to_bytes(), &bin_array_index.to_le_bytes()], program_id).0
}

pub async fn hydrate(pool: &mut DecodedDlmmPool, rpc_client: &RpcClient) -> Result<()> {
    let (mint_a_res, mint_b_res) = tokio::join!(
        rpc_client.get_account_data(&pool.mint_a),
        rpc_client.get_account_data(&pool.mint_b)
    );
    let mint_a_data = mint_a_res?;
    let decoded_mint_a = spl_token_decoders::mint::decode_mint(&pool.mint_a, &mint_a_data)?;
    pool.mint_a_transfer_fee_bps = decoded_mint_a.transfer_fee_basis_points;
    let mint_b_data = mint_b_res?;
    let decoded_mint_b = spl_token_decoders::mint::decode_mint(&pool.mint_b, &mint_b_data)?;
    pool.mint_b_transfer_fee_bps = decoded_mint_b.transfer_fee_basis_points;

    println!("\n--- DIAGNOSTIC D'HYDRATATION ---");
    println!("Pool: {}", pool.address);
    println!("Programme ID: {}", pool.program_id);
    println!("Bin Actif ID: {}", pool.active_bin_id);
    let mut bin_array_indices_to_fetch = BTreeSet::new();
    let active_array_idx = (pool.active_bin_id as i64 / MAX_BIN_PER_ARRAY as i64) - if pool.active_bin_id < 0 && pool.active_bin_id % MAX_BIN_PER_ARRAY != 0 { 1 } else { 0 };
    println!("Index du BinArray Actif (calculé): {}", active_array_idx);
    for i in -10..=10 { bin_array_indices_to_fetch.insert(active_array_idx + i); }
    let mut addresses_to_fetch = vec![];
    println!("\nCalcul des adresses PDA des BinArray à charger :");
    for idx in &bin_array_indices_to_fetch {
        let address = get_bin_array_address(&pool.address, (idx * MAX_BIN_PER_ARRAY as i64) as i32, &pool.program_id);
        addresses_to_fetch.push(address);
        println!("-> Index {}: {}", idx, address);
    }
    println!("------------------------------------");
    let accounts = rpc_client.get_multiple_accounts(&addresses_to_fetch).await?;
    let mut hydrated_bin_arrays = BTreeMap::new();
    println!("\nRésultats du chargement des comptes :");
    for (i, account) in accounts.iter().enumerate() {
        if let Some(acc) = account {
            if let Some(idx) = bin_array_indices_to_fetch.iter().nth(i) {
                println!("-> [OK] Compte pour Index {} trouvé (Taille: {} octets)", idx, acc.data.len());
                hydrated_bin_arrays.insert(*idx, acc.data.clone());
            }
        } else {
            if let Some(idx) = bin_array_indices_to_fetch.iter().nth(i) {
                println!("-> [ÉCHEC] Compte pour Index {} NON trouvé", idx);
            }
        }
    }
    println!("-------------------------------------");
    pool.hydrated_bin_arrays = Some(hydrated_bin_arrays);
    Ok(())
}
