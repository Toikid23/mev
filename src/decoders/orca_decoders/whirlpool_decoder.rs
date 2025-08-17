// DANS: src/decoders/orca_decoders/whirlpool_decoder.rs

use crate::decoders::pool_operations::PoolOperations;
use bytemuck::{Pod, Zeroable, from_bytes};
use solana_sdk::pubkey::Pubkey;
use anyhow::{anyhow, Result, bail};
use std::collections::BTreeMap;
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::pubkey;

use crate::decoders::spl_token_decoders;
use crate::math::orca_whirlpool_math;
use super::tick_array;
// --- STRUCTURE DE TRAVAIL "PROPRE" (MODIFIÉE) ---
#[derive(Debug, Clone)]
pub struct DecodedWhirlpoolPool {
    pub address: Pubkey,
    pub program_id: Pubkey,
    pub whirlpools_config: Pubkey,

    // Mints & Vaults
    pub mint_a: Pubkey,
    pub mint_b: Pubkey,
    pub vault_a: Pubkey,
    pub vault_b: Pubkey,

    // Données de l'état du pool
    pub liquidity: u128,
    pub sqrt_price: u128,
    pub tick_current_index: i32,
    pub tick_spacing: u16,

    // Frais
    pub fee_rate: u16,
    pub protocol_fee_rate: u16,

    // Données à hydrater
    pub mint_a_decimals: u8,
    pub mint_b_decimals: u8,
    pub mint_a_transfer_fee_bps: u16,
    pub mint_b_transfer_fee_bps: u16,
    // --- CHAMP AJOUTÉ POUR LES TICKS ---
    // Un BTreeMap est parfait pour stocker les tick arrays de manière ordonnée.
    pub tick_arrays: Option<BTreeMap<i32, tick_array::TickArrayData>>,
    pub mint_a_program: Pubkey,
    pub mint_b_program: Pubkey,
}

impl DecodedWhirlpoolPool {
    pub fn fee_as_percent(&self) -> f64 {
        // La fee_rate est en parts par million.
        // Pour la convertir en pourcentage, on divise par 10,000.
        self.fee_rate as f64 / 10_000.0
    }
}
// --- STRUCTURES BRUTES (INCHANGÉES) ---
#[repr(C, packed)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
pub struct WhirlpoolRewardInfoData {
    pub mint: Pubkey, pub vault: Pubkey, pub authority: Pubkey,
    pub emissions_per_second_x64: u128, pub growth_global_x64: u128,
}

#[repr(C, packed)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
pub struct WhirlpoolData {
    pub whirlpools_config: Pubkey, pub whirlpool_bump: [u8; 1], pub tick_spacing: u16,
    pub fee_tier_index_seed: [u8; 2], pub fee_rate: u16, pub protocol_fee_rate: u16,
    pub liquidity: u128, pub sqrt_price: u128, pub tick_current_index: i32,
    pub protocol_fee_owed_a: u64, pub protocol_fee_owed_b: u64,
    pub token_mint_a: Pubkey, pub token_vault_a: Pubkey, pub fee_growth_global_a: u128,
    pub token_mint_b: Pubkey, pub token_vault_b: Pubkey, pub fee_growth_global_b: u128,
    pub reward_last_updated_timestamp: u64,
    pub reward_infos: [WhirlpoolRewardInfoData; 3],
}

// --- FONCTION DE DÉCODAGE (MODIFIÉE POUR INCLURE LES PLACEHOLDERS) ---
pub fn decode_pool(address: &Pubkey, data: &[u8]) -> Result<DecodedWhirlpoolPool> {
    println!("DEBUG: Discriminateur reçu du RPC: {:?}", &data[..8]);
    const DISCRIMINATOR: [u8; 8] = [63, 149, 209, 12, 225, 128, 99, 9];
    if data.get(..8) != Some(&DISCRIMINATOR) {
        bail!("Invalid discriminator. Not a Whirlpool account.");
    }

    let data_slice = &data[8..];
    let expected_size = std::mem::size_of::<WhirlpoolData>();
    if data_slice.len() < expected_size {
        bail!("Whirlpool data length mismatch.");
    }

    let pool_struct: &WhirlpoolData = from_bytes(&data_slice[..expected_size]);

    Ok(DecodedWhirlpoolPool {
        address: *address,
        program_id: pubkey!("whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc"),
        whirlpools_config: pool_struct.whirlpools_config,
        mint_a: pool_struct.token_mint_a, mint_b: pool_struct.token_mint_b,
        vault_a: pool_struct.token_vault_a, vault_b: pool_struct.token_vault_b,
        liquidity: pool_struct.liquidity, sqrt_price: pool_struct.sqrt_price,
        tick_current_index: pool_struct.tick_current_index,
        tick_spacing: pool_struct.tick_spacing,
        fee_rate: pool_struct.fee_rate, protocol_fee_rate: pool_struct.protocol_fee_rate,
        mint_a_decimals: 0, mint_b_decimals: 0,
        mint_a_transfer_fee_bps: 0, mint_b_transfer_fee_bps: 0,
        tick_arrays: None, // Initialisé à None
        mint_a_program: spl_token::id(), // Valeur par défaut
        mint_b_program: spl_token::id(),
    })
}

// --- IMPLÉMENTATION DE LA FONCTION D'HYDRATATION (NOUVEAU) ---
pub async fn hydrate(pool: &mut DecodedWhirlpoolPool, rpc_client: &RpcClient) -> Result<()> {
    // Étape 1: Hydrater les mints
    let mints_to_fetch = [pool.mint_a, pool.mint_b];
    let mint_accounts = rpc_client.get_multiple_accounts(&mints_to_fetch).await?;

    let mint_a_account = mint_accounts[0].as_ref().ok_or_else(|| anyhow!("Mint A not found"))?;
    pool.mint_a_program = mint_a_account.owner; // <-- ON AJOUTE CETTE LIGNE
    let decoded_mint_a = spl_token_decoders::mint::decode_mint(&pool.mint_a, &mint_a_account.data)?;
    pool.mint_a_decimals = decoded_mint_a.decimals;
    pool.mint_a_transfer_fee_bps = decoded_mint_a.transfer_fee_basis_points;

    // Traitement pour le Mint B
    let mint_b_account = mint_accounts[1].as_ref().ok_or_else(|| anyhow!("Mint B not found"))?;
    pool.mint_b_program = mint_b_account.owner; // <-- ON AJOUTE CETTE LIGNE
    let decoded_mint_b = spl_token_decoders::mint::decode_mint(&pool.mint_b, &mint_b_account.data)?;
    pool.mint_b_decimals = decoded_mint_b.decimals;
    pool.mint_b_transfer_fee_bps = decoded_mint_b.transfer_fee_basis_points;
    // Étape 2: Préparer et fetcher les TickArrays
    let active_array_start_index = tick_array::get_start_tick_index(pool.tick_current_index, pool.tick_spacing);
    let ticks_per_array = (tick_array::TICK_ARRAY_SIZE as i32) * (pool.tick_spacing as i32);

    let mut all_addresses_to_fetch = Vec::new();
    const WINDOW_SIZE: i32 = 5;
    for i in -WINDOW_SIZE..=WINDOW_SIZE {
        let start_tick = active_array_start_index + (i * ticks_per_array);
        all_addresses_to_fetch.push(tick_array::get_tick_array_address(&pool.address, start_tick, &pool.program_id));
    }

    let tick_array_accounts = rpc_client.get_multiple_accounts(&all_addresses_to_fetch).await?;

    // Étape 3: Traiter les résultats
    let mut hydrated_tick_arrays = BTreeMap::new();
    for account_opt in tick_array_accounts {
        if let Some(account) = account_opt {
            if let Ok(decoded_array) = tick_array::decode_tick_array(&account.data) {
                hydrated_tick_arrays.insert(decoded_array.start_tick_index, decoded_array);
            }
        }
    }
    pool.tick_arrays = Some(hydrated_tick_arrays);

    Ok(())
}


fn find_next_initialized_tick<'a>(
    pool: &'a DecodedWhirlpoolPool,
    current_tick_index: i32,
    a_to_b: bool,
    tick_arrays: &'a BTreeMap<i32, tick_array::TickArrayData>,
) -> Option<(i32, &'a tick_array::TickData)> {
    let tick_spacing = pool.tick_spacing as i32;

    if a_to_b { // Le prix baisse, on cherche un tick inférieur
        let current_array_start_index = tick_array::get_start_tick_index(current_tick_index, pool.tick_spacing);

        // On commence par chercher dans le reste de l'array actuel
        if let Some(array_data) = tick_arrays.get(&current_array_start_index) {
            let array_offset = ((current_tick_index - current_array_start_index) / tick_spacing).clamp(0, tick_array::TICK_ARRAY_SIZE as i32 - 1);
            for i in (0..array_offset).rev() {
                let tick_data = &array_data.ticks[i as usize];
                if tick_data.initialized == 1 && tick_data.liquidity_net != 0 {
                    return Some((current_array_start_index + i * tick_spacing, tick_data));
                }
            }
        }

        // Puis on itère sur les arrays précédents
        for (start_tick, array_data) in tick_arrays.range(..current_array_start_index).rev() {
            for i in (0..tick_array::TICK_ARRAY_SIZE).rev() {
                let tick_data = &array_data.ticks[i];
                if tick_data.initialized == 1 && tick_data.liquidity_net != 0 {
                    return Some((*start_tick + (i as i32) * tick_spacing, tick_data));
                }
            }
        }
    } else { // Le prix monte, on cherche un tick supérieur
        let current_array_start_index = tick_array::get_start_tick_index(current_tick_index, pool.tick_spacing);

        // On commence par la fin de l'array actuel
        if let Some(array_data) = tick_arrays.get(&current_array_start_index) {
            let array_offset = ((current_tick_index - current_array_start_index) / tick_spacing).clamp(0, tick_array::TICK_ARRAY_SIZE as i32 -1);
            for i in (array_offset + 1)..(tick_array::TICK_ARRAY_SIZE as i32) {
                let tick_data = &array_data.ticks[i as usize];
                if tick_data.initialized == 1 && tick_data.liquidity_net != 0 {
                    return Some((current_array_start_index + i * tick_spacing, tick_data));
                }
            }
        }

        // Puis on itère sur les arrays suivants
        for (start_tick, array_data) in tick_arrays.range(current_array_start_index + 1..) {
            for i in 0..tick_array::TICK_ARRAY_SIZE {
                let tick_data = &array_data.ticks[i];
                if tick_data.initialized == 1 && tick_data.liquidity_net != 0 {
                    return Some((*start_tick + (i as i32) * tick_spacing, tick_data));
                }
            }
        }
    }
    None
}

fn calculate_swap(
    pool: &DecodedWhirlpoolPool,
    amount_in: u64,
    a_to_b: bool,
    tick_arrays: &BTreeMap<i32, tick_array::TickArrayData>,
) -> Result<u64> {
    let mut amount_remaining = amount_in as u128;
    let mut total_amount_out: u128 = 0;

    let mut current_liquidity = pool.liquidity;
    let mut current_sqrt_price = pool.sqrt_price;
    let mut current_tick_index = pool.tick_current_index;

    while amount_remaining > 0 && current_liquidity > 0 {
        let next_initialized_tick = find_next_initialized_tick(pool, current_tick_index, a_to_b, tick_arrays);

        // CORRECTION E0609: On utilise l'index retourné pour calculer le sqrt_price
        let sqrt_price_target = if let Some((tick_index, _)) = next_initialized_tick {
            orca_whirlpool_math::tick_to_sqrt_price_x64(tick_index)
        } else {
            if a_to_b { 0 } else { u128::MAX }
        };

        let (amount_in_step, amount_out_step, next_sqrt_price) = orca_whirlpool_math::compute_swap_step(
            amount_remaining, current_sqrt_price, sqrt_price_target, current_liquidity, a_to_b,
        );

        amount_remaining -= amount_in_step;
        total_amount_out += amount_out_step;
        current_sqrt_price = next_sqrt_price;

        if current_sqrt_price == sqrt_price_target {
            // CORRECTION E0609: On utilise les données du tick retourné
            if let Some((tick_index, tick_data)) = next_initialized_tick {
                current_liquidity = (current_liquidity as i128 + tick_data.liquidity_net) as u128;
                current_tick_index = tick_index;
            } else {
                break;
            }
        }
    }

    Ok(total_amount_out as u64)
}

// --- IMPLÉMENTATION FINALE DU TRAIT POOLOPERATIONS ---
impl PoolOperations for DecodedWhirlpoolPool {
    fn get_mints(&self) -> (Pubkey, Pubkey) { (self.mint_a, self.mint_b) }
    fn get_vaults(&self) -> (Pubkey, Pubkey) { (self.vault_a, self.vault_b) }

    fn get_quote(&self, token_in_mint: &Pubkey, amount_in: u64, _current_timestamp: i64) -> Result<u64> {
        let tick_arrays = self.tick_arrays.as_ref().ok_or_else(|| anyhow!("Pool is not hydrated."))?;
        if tick_arrays.is_empty() { return Ok(0); }

        let a_to_b = *token_in_mint == self.mint_a;

        // --- DÉBUT DE LA CORRECTION DE LA LOGIQUE DES FRAIS ---

        // 1. Appliquer les frais de transfert Token-2022 sur l'input (votre logique est déjà bonne)
        let (in_mint_fee_bps, out_mint_fee_bps) = if a_to_b {
            (self.mint_a_transfer_fee_bps, self.mint_b_transfer_fee_bps)
        } else {
            (self.mint_b_transfer_fee_bps, self.mint_a_transfer_fee_bps)
        };
        let fee_on_input_token2022 = (amount_in as u128 * in_mint_fee_bps as u128) / 10000;
        let amount_in_after_token2022_fee = amount_in.saturating_sub(fee_on_input_token2022 as u64);

        // 2. Appliquer les frais de pool (LP) sur le montant d'entrée net.
        // La `fee_rate` est en 1/1,000,000.
        let pool_fee = (amount_in_after_token2022_fee as u128 * self.fee_rate as u128) / 1_000_000;
        let amount_to_swap = amount_in_after_token2022_fee.saturating_sub(pool_fee as u64);

        // 3. Calculer le swap avec le montant net de frais.
        let gross_amount_out = calculate_swap(self, amount_to_swap, a_to_b, tick_arrays)?;

        // 4. Appliquer les frais de transfert Token-2022 sur l'output (votre logique est déjà bonne)
        let fee_on_output_token2022 = (gross_amount_out as u128 * out_mint_fee_bps as u128) / 10000;
        let final_amount_out = gross_amount_out.saturating_sub(fee_on_output_token2022 as u64);

        Ok(final_amount_out)

        // --- FIN DE LA CORRECTION ---
    }
}