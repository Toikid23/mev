// DANS: src/decoders/orca_decoders/pool

use bytemuck::{Pod, Zeroable, from_bytes};
use solana_sdk::pubkey::Pubkey;
use anyhow::{anyhow, Result, bail};
use std::collections::BTreeMap;
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::pubkey;
use serde::{Serialize, Deserialize};
use crate::decoders::spl_token_decoders;
use super::math as orca_whirlpool_math;
use super::tick_array;
use super::math::sqrt_price_to_tick_index;
use async_trait::async_trait;
use crate::decoders::pool_operations::{PoolOperations, UserSwapAccounts};
use solana_sdk::instruction::{Instruction, AccountMeta};
use spl_associated_token_account::get_associated_token_address;

// --- STRUCTURE DE TRAVAIL "PROPRE" (MODIFIÉE) ---
#[derive(Debug, Clone, Serialize, Deserialize)]
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
    pub last_swap_timestamp: i64,
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
        last_swap_timestamp: 0,
    })
}

pub async fn hydrate(pool: &mut DecodedWhirlpoolPool, rpc_client: &RpcClient) -> Result<()> {
    // --- Étape 1: Hydrater les mints (logique inchangée et correcte) ---
    let (mint_a_res, mint_b_res) = tokio::join!(
        rpc_client.get_account(&pool.mint_a),
        rpc_client.get_account(&pool.mint_b)
    );

    let mint_a_account = mint_a_res?;
    pool.mint_a_program = mint_a_account.owner;
    let decoded_mint_a = spl_token_decoders::mint::decode_mint(&pool.mint_a, &mint_a_account.data)?;
    pool.mint_a_decimals = decoded_mint_a.decimals;
    pool.mint_a_transfer_fee_bps = decoded_mint_a.transfer_fee_basis_points;

    let mint_b_account = mint_b_res?;
    pool.mint_b_program = mint_b_account.owner;
    let decoded_mint_b = spl_token_decoders::mint::decode_mint(&pool.mint_b, &mint_b_account.data)?;
    pool.mint_b_decimals = decoded_mint_b.decimals;
    pool.mint_b_transfer_fee_bps = decoded_mint_b.transfer_fee_basis_points;

    // --- Étape 2: Hydratation "Look-Ahead" des TickArrays ---
    let mut tick_arrays_to_fetch = std::collections::HashSet::new();
    let tick_spacing_i32 = pool.tick_spacing as i32;
    let ticks_in_one_array = tick_array::TICK_ARRAY_SIZE as i32 * tick_spacing_i32;

    // On calcule l'index de l'array actuel, celui d'avant, et celui d'après.
    let current_array_start_index = tick_array::get_start_tick_index(pool.tick_current_index, pool.tick_spacing);
    let prev_array_start_index = current_array_start_index - ticks_in_one_array;
    let next_array_start_index = current_array_start_index + ticks_in_one_array;

    // On ajoute leurs adresses à la liste des comptes à récupérer.
    tick_arrays_to_fetch.insert(tick_array::get_tick_array_address(&pool.address, prev_array_start_index, &pool.program_id));
    tick_arrays_to_fetch.insert(tick_array::get_tick_array_address(&pool.address, current_array_start_index, &pool.program_id));
    tick_arrays_to_fetch.insert(tick_array::get_tick_array_address(&pool.address, next_array_start_index, &pool.program_id));

    // On fait UN SEUL appel RPC groupé pour récupérer les 3 comptes. C'est très efficace.
    let accounts_results = rpc_client.get_multiple_accounts(&tick_arrays_to_fetch.into_iter().collect::<Vec<_>>()).await?;

    let mut hydrated_tick_arrays = BTreeMap::new();
    for account_opt in accounts_results {
        if let Some(account) = account_opt {
            if let Ok(decoded_array) = tick_array::decode_tick_array(&account.data) {
                hydrated_tick_arrays.insert(decoded_array.start_tick_index, decoded_array);
            }
        }
    }

    pool.tick_arrays = Some(hydrated_tick_arrays);

    Ok(())
}

pub async fn hydrate_with_depth(pool: &mut DecodedWhirlpoolPool, rpc_client: &RpcClient, depth: usize) -> Result<()> {
    // Étape 1: Hydrater les mints (inchangé)
    let (mint_a_res, mint_b_res) = tokio::join!(
        rpc_client.get_account(&pool.mint_a),
        rpc_client.get_account(&pool.mint_b)
    );
    // ... (le reste de l'hydratation des mints est correct et ne change pas)
    let mint_a_account = mint_a_res?;
    pool.mint_a_program = mint_a_account.owner;
    let decoded_mint_a = spl_token_decoders::mint::decode_mint(&pool.mint_a, &mint_a_account.data)?;
    pool.mint_a_decimals = decoded_mint_a.decimals;
    pool.mint_a_transfer_fee_bps = decoded_mint_a.transfer_fee_basis_points;
    let mint_b_account = mint_b_res?;
    pool.mint_b_program = mint_b_account.owner;
    let decoded_mint_b = spl_token_decoders::mint::decode_mint(&pool.mint_b, &mint_b_account.data)?;
    pool.mint_b_decimals = decoded_mint_b.decimals;
    pool.mint_b_transfer_fee_bps = decoded_mint_b.transfer_fee_basis_points;

    // Étape 2: Hydratation "Look-Ahead" avec profondeur variable
    let mut tick_arrays_to_fetch = std::collections::HashSet::new();
    let tick_spacing_i32 = pool.tick_spacing as i32;
    let ticks_in_one_array = tick_array::TICK_ARRAY_SIZE as i32 * tick_spacing_i32;
    let current_array_start_index = tick_array::get_start_tick_index(pool.tick_current_index, pool.tick_spacing);

    // On charge l'array actuel
    tick_arrays_to_fetch.insert(tick_array::get_tick_array_address(&pool.address, current_array_start_index, &pool.program_id));

    // On charge `depth` arrays de chaque côté
    for i in 1..=depth {
        let prev_offset = (i as i32) * ticks_in_one_array;
        let next_offset = (i as i32) * ticks_in_one_array;
        tick_arrays_to_fetch.insert(tick_array::get_tick_array_address(&pool.address, current_array_start_index - prev_offset, &pool.program_id));
        tick_arrays_to_fetch.insert(tick_array::get_tick_array_address(&pool.address, current_array_start_index + next_offset, &pool.program_id));
    }

    let accounts_results = rpc_client.get_multiple_accounts(&tick_arrays_to_fetch.into_iter().collect::<Vec<_>>()).await?;

    let mut hydrated_tick_arrays = BTreeMap::new();
    for account_opt in accounts_results {
        if let Some(account) = account_opt {
            if let Ok(decoded_array) = tick_array::decode_tick_array(&account.data) {
                hydrated_tick_arrays.insert(decoded_array.start_tick_index, decoded_array);
            }
        }
    }

    pool.tick_arrays = Some(hydrated_tick_arrays);
    Ok(())
}

pub async fn rehydrate_for_escalation(
    pool: &mut DecodedWhirlpoolPool,
    rpc_client: &RpcClient,
    go_up: bool, // true si on a besoin d'index de tick plus élevés
) -> Result<()> {
    if pool.tick_arrays.is_none() {
        // Fallback au cas où le pool n'aurait pas été hydraté du tout
        return hydrate_with_depth(pool, rpc_client, 1).await;
    }

    let tick_arrays = pool.tick_arrays.as_mut().unwrap();

    // On trouve l'index de l'array le plus haut ou le plus bas déjà chargé
    let boundary_index = if go_up {
        *tick_arrays.keys().max().unwrap_or(&0)
    } else {
        *tick_arrays.keys().min().unwrap_or(&0)
    };

    let ticks_in_one_array = tick_array::TICK_ARRAY_SIZE as i32 * pool.tick_spacing as i32;
    let mut new_indices_to_fetch = vec![];

    // On va chercher les 3 arrays suivants dans la direction demandée
    for i in 1..=3 {
        let next_index = if go_up {
            boundary_index + (i * ticks_in_one_array)
        } else {
            boundary_index - (i * ticks_in_one_array)
        };
        new_indices_to_fetch.push(next_index);
    }

    // On fait les appels RPC pour les nouveaux arrays
    for &index in &new_indices_to_fetch {
        let address = tick_array::get_tick_array_address(&pool.address, index, &pool.program_id);
        if let Ok(account) = rpc_client.get_account(&address).await {
            if let Ok(decoded_array) = tick_array::decode_tick_array(&account.data) {
                // On ajoute les nouveaux arrays au BTreeMap existant, étendant notre connaissance
                tick_arrays.insert(index, decoded_array);
            }
        }
    }

    Ok(())
}

fn find_next_initialized_tick<'a>(
    tick_spacing: u16,
    current_tick_index: i32,
    a_to_b: bool,
    tick_arrays: &'a BTreeMap<i32, tick_array::TickArrayData>,
) -> Option<(i32, &'a tick_array::TickData)> {
    let tick_spacing = tick_spacing as i32;

    if a_to_b { // Le prix baisse, on cherche un tick inférieur
        let current_array_start_index = tick_array::get_start_tick_index(current_tick_index, tick_spacing as u16);

        if let Some(array_data) = tick_arrays.get(&current_array_start_index) {
            let array_offset = ((current_tick_index - current_array_start_index) / tick_spacing).clamp(0, tick_array::TICK_ARRAY_SIZE as i32 - 1);
            // On cherche le prochain tick initialisé *avant* l'offset actuel.
            // La boucle commence à array_offset - 1.
            for i in (0..array_offset).rev() {
                let tick_data = &array_data.ticks[i as usize];
                // La seule condition est que le tick soit initialisé.
                if tick_data.initialized == 1 {
                    return Some((current_array_start_index + i * tick_spacing, tick_data));
                }
            }
        }

        // On continue sur les arrays précédents
        for (start_tick, array_data) in tick_arrays.range(..current_array_start_index).rev() {
            for i in (0..tick_array::TICK_ARRAY_SIZE).rev() {
                let tick_data = &array_data.ticks[i];
                if tick_data.initialized == 1 {
                    return Some((*start_tick + (i as i32) * tick_spacing, tick_data));
                }
            }
        }
    } else { // Le prix monte, on cherche un tick supérieur
        let current_array_start_index = tick_array::get_start_tick_index(current_tick_index, tick_spacing as u16);

        if let Some(array_data) = tick_arrays.get(&current_array_start_index) {
            let array_offset = ((current_tick_index - current_array_start_index) / tick_spacing).clamp(0, tick_array::TICK_ARRAY_SIZE as i32 - 1);
            // On cherche le prochain tick initialisé *après* l'offset actuel
            for i in (array_offset + 1)..(tick_array::TICK_ARRAY_SIZE as i32) {
                let tick_data = &array_data.ticks[i as usize];
                if tick_data.initialized == 1 {
                    return Some((current_array_start_index + i * tick_spacing, tick_data));
                }
            }
        }

        // On continue sur les arrays suivants
        for (start_tick, array_data) in tick_arrays.range(current_array_start_index + 1..) {
            for i in 0..tick_array::TICK_ARRAY_SIZE {
                let tick_data = &array_data.ticks[i];
                if tick_data.initialized == 1 {
                    return Some((*start_tick + (i as i32) * tick_spacing, tick_data));
                }
            }
        }
    }
    None
}


impl DecodedWhirlpoolPool {
    // Fonctions utilitaires
    pub fn fee_as_percent(&self) -> f64 { self.fee_rate as f64 / 10_000.0 }

}

#[async_trait]
impl PoolOperations for DecodedWhirlpoolPool {
    fn get_mints(&self) -> (Pubkey, Pubkey) {
        (self.mint_a, self.mint_b)
    }

    fn get_vaults(&self) -> (Pubkey, Pubkey) {
        (self.vault_a, self.vault_b)
    }

    fn get_reserves(&self) -> (u64, u64) {
        // Les CLMM n'ont pas de réserves simples. On retourne 0 pour que la stratégie utilise son fallback.
        (0, 0)
    }

    fn address(&self) -> Pubkey { self.address }

    // NOUVELLE VERSION SYNCHRONE DE get_quote
    fn get_quote(&self, token_in_mint: &Pubkey, amount_in: u64, _current_timestamp: i64) -> Result<u64> {
        let tick_arrays = self.tick_arrays.as_ref().ok_or_else(|| anyhow!("Pool is not hydrated."))?;
        if self.liquidity == 0 && tick_arrays.is_empty() { return Ok(0); }

        let a_to_b = *token_in_mint == self.mint_a;
        let (in_mint_fee_bps, out_mint_fee_bps) = if a_to_b {
            (self.mint_a_transfer_fee_bps, self.mint_b_transfer_fee_bps)
        } else {
            (self.mint_b_transfer_fee_bps, self.mint_a_transfer_fee_bps)
        };

        let fee_on_input = (amount_in as u128 * in_mint_fee_bps as u128) / 10000;
        let amount_in_after_fee = amount_in.saturating_sub(fee_on_input as u64);

        let pool_fee = (amount_in_after_fee as u128 * self.fee_rate as u128).div_ceil(1_000_000);
        let mut amount_remaining = amount_in_after_fee.saturating_sub(pool_fee as u64) as u128;

        let mut total_amount_out: u128 = 0;
        let mut current_liquidity = self.liquidity;
        let mut current_sqrt_price = self.sqrt_price;
        let mut current_tick_index = self.tick_current_index;

        while amount_remaining > 0 && current_liquidity > 0 {
            let (target_sqrt_price, next_liquidity_net) = {
                match find_next_initialized_tick(self.tick_spacing, current_tick_index, a_to_b, tick_arrays) {
                    Some((tick_index, tick_data)) => (orca_whirlpool_math::tick_to_sqrt_price_x64(tick_index), Some(tick_data.liquidity_net)),
                    None => (if a_to_b { orca_whirlpool_math::MIN_SQRT_PRICE_X64 } else { orca_whirlpool_math::MAX_SQRT_PRICE_X64 }, None)
                }
            };

            let (amount_in_step, amount_out_step, next_sqrt_price) = orca_whirlpool_math::compute_swap_step(
                amount_remaining, current_sqrt_price, target_sqrt_price, current_liquidity, a_to_b,
            );

            if amount_in_step == 0 { break; } // On ne progresse plus

            amount_remaining -= amount_in_step;
            total_amount_out += amount_out_step;
            current_sqrt_price = next_sqrt_price;

            if current_sqrt_price == target_sqrt_price {
                if let Some(liquidity_net) = next_liquidity_net {
                    current_liquidity = (current_liquidity as i128 + liquidity_net) as u128;
                    current_tick_index = sqrt_price_to_tick_index(current_sqrt_price);
                } else { break; }
            }
        }

        let fee_on_output = (total_amount_out * out_mint_fee_bps as u128) / 10000;
        let final_amount_out = total_amount_out.saturating_sub(fee_on_output);

        Ok(final_amount_out as u64)
    }

    // NOUVELLE VERSION SYNCHRONE DE get_required_input
    fn get_required_input(&mut self, token_out_mint: &Pubkey, amount_out: u64, _current_timestamp: i64) -> Result<u64> {
        let tick_arrays = self.tick_arrays.as_ref().ok_or_else(|| anyhow!("Pool is not hydrated."))?;
        if amount_out == 0 { return Ok(0); }
        if self.liquidity == 0 && tick_arrays.is_empty() { return Err(anyhow!("Pool has no liquidity.")); }

        let a_to_b = *token_out_mint == self.mint_b;
        let (in_mint_fee_bps, out_mint_fee_bps) = if a_to_b {
            (self.mint_a_transfer_fee_bps, self.mint_b_transfer_fee_bps)
        } else {
            (self.mint_b_transfer_fee_bps, self.mint_a_transfer_fee_bps)
        };

        const BPS_DENOMINATOR: u128 = 10000;
        let mut gross_amount_out_target = if out_mint_fee_bps > 0 {
            let numerator = (amount_out as u128).saturating_mul(BPS_DENOMINATOR);
            let denominator = BPS_DENOMINATOR.saturating_sub(out_mint_fee_bps as u128);
            numerator.div_ceil(denominator)
        } else {
            amount_out as u128
        };

        let mut total_amount_in_net: u128 = 0;
        let mut current_sqrt_price = self.sqrt_price;
        let mut current_tick_index = self.tick_current_index;
        let mut current_liquidity = self.liquidity;
        let input_a_to_b = !a_to_b;

        while gross_amount_out_target > 0 {
            if current_liquidity == 0 {
                // ... (la logique de gestion de liquidité nulle reste identique)
                let find_result = find_next_initialized_tick(self.tick_spacing, current_tick_index, input_a_to_b, tick_arrays);
                if let Some((next_tick_idx, tick_data)) = find_result {
                    current_tick_index = if input_a_to_b { next_tick_idx - 1 } else { next_tick_idx };
                    current_sqrt_price = orca_whirlpool_math::tick_to_sqrt_price_x64(current_tick_index);
                    current_liquidity = (current_liquidity as i128 + tick_data.liquidity_net) as u128;
                    continue;
                } else {
                    return Err(anyhow!("Not enough liquidity to reach target amount out."));
                }
            }

            let (next_tick_index, next_liquidity_net) = match find_next_initialized_tick(self.tick_spacing, current_tick_index, input_a_to_b, tick_arrays) {
                Some((tick_index, tick_data)) => (tick_index, tick_data.liquidity_net),
                None => (if input_a_to_b { -443636 } else { 443636 }, 0)
            };

            let sqrt_price_target = orca_whirlpool_math::tick_to_sqrt_price_x64(next_tick_index);

            let amount_out_available_in_step = if a_to_b {
                orca_whirlpool_math::get_delta_y(sqrt_price_target, current_sqrt_price, current_liquidity)
            } else {
                orca_whirlpool_math::get_delta_x(current_sqrt_price, sqrt_price_target, current_liquidity)
            };

            let amount_out_chunk = gross_amount_out_target.min(amount_out_available_in_step);

            if amount_out_chunk > 0 {
                let (prev_sqrt_price, amount_in_step_net) = if a_to_b {
                    let p_start = orca_whirlpool_math::get_next_sqrt_price_y_down(current_sqrt_price, current_liquidity, amount_out_chunk);
                    (p_start, orca_whirlpool_math::get_delta_x_ceil(p_start, current_sqrt_price, current_liquidity))
                } else {
                    let p_start = orca_whirlpool_math::get_next_sqrt_price_x_up(current_sqrt_price, current_liquidity, amount_out_chunk);
                    (p_start, orca_whirlpool_math::get_delta_y_ceil(current_sqrt_price, p_start, current_liquidity))
                };
                total_amount_in_net += amount_in_step_net;
                current_sqrt_price = prev_sqrt_price;
            }

            gross_amount_out_target -= amount_out_chunk;

            if amount_out_chunk == amount_out_available_in_step {
                current_liquidity = (current_liquidity as i128 + next_liquidity_net) as u128;
                current_tick_index = if input_a_to_b { next_tick_index - 1 } else { next_tick_index };
            }
        }

        const FEE_RATE_DENOMINATOR: u128 = 1_000_000;
        let amount_in_after_transfer_fee = if self.fee_rate > 0 {
            let num = total_amount_in_net.saturating_mul(FEE_RATE_DENOMINATOR);
            let den = FEE_RATE_DENOMINATOR.saturating_sub(self.fee_rate as u128);
            num.div_ceil(den)
        } else {
            total_amount_in_net
        };

        let mut final_amount_in = if in_mint_fee_bps > 0 {
            let numerator = amount_in_after_transfer_fee.saturating_mul(BPS_DENOMINATOR);
            let denominator = BPS_DENOMINATOR.saturating_sub(in_mint_fee_bps as u128);
            numerator.div_ceil(denominator)
        } else {
            amount_in_after_transfer_fee
        };

        final_amount_in = final_amount_in.saturating_add(5);

        Ok(final_amount_in as u64)
    }


    fn create_swap_instruction(
        &self,
        token_in_mint: &Pubkey,
        amount_in: u64,
        min_amount_out: u64,
        user_accounts: &UserSwapAccounts,
    ) -> Result<Instruction> {
        let a_to_b = *token_in_mint == self.mint_a;

        let ticks_in_array = (tick_array::TICK_ARRAY_SIZE as i32) * (self.tick_spacing as i32);
        let current_array_start_index = tick_array::get_start_tick_index(self.tick_current_index, self.tick_spacing);

        let tick_array_addresses: [Pubkey; 3] = if a_to_b {
            [
                tick_array::get_tick_array_address(&self.address, current_array_start_index, &self.program_id),
                tick_array::get_tick_array_address(&self.address, current_array_start_index - ticks_in_array, &self.program_id),
                tick_array::get_tick_array_address(&self.address, current_array_start_index - (2 * ticks_in_array), &self.program_id),
            ]
        } else {
            [
                tick_array::get_tick_array_address(&self.address, current_array_start_index, &self.program_id),
                tick_array::get_tick_array_address(&self.address, current_array_start_index + ticks_in_array, &self.program_id),
                tick_array::get_tick_array_address(&self.address, current_array_start_index + (2 * ticks_in_array), &self.program_id),
            ]
        };

        let sqrt_price_limit: u128 = if a_to_b { 4295048016 } else { 79226673515401279992447579055 };

        let swap_discriminator: [u8; 8] = [248, 198, 158, 145, 225, 117, 135, 200];
        let mut instruction_data = Vec::new();
        instruction_data.extend_from_slice(&swap_discriminator);
        instruction_data.extend_from_slice(&amount_in.to_le_bytes());
        instruction_data.extend_from_slice(&min_amount_out.to_le_bytes());
        instruction_data.extend_from_slice(&sqrt_price_limit.to_le_bytes());
        instruction_data.push(u8::from(true));
        instruction_data.push(u8::from(a_to_b));

        let (oracle_pda, _) = Pubkey::find_program_address(&[b"oracle", self.address.as_ref()], &self.program_id);

        let user_ata_for_token_a = get_associated_token_address(&user_accounts.owner, &self.mint_a);
        let user_ata_for_token_b = get_associated_token_address(&user_accounts.owner, &self.mint_b);

        let mut accounts = vec![
            AccountMeta::new_readonly(spl_token::id(), false),
            AccountMeta::new_readonly(user_accounts.owner, true),
            AccountMeta::new(self.address, false),
            AccountMeta::new(user_ata_for_token_a, false),
            AccountMeta::new(self.vault_a, false),
            AccountMeta::new(user_ata_for_token_b, false),
            AccountMeta::new(self.vault_b, false),
            AccountMeta::new(tick_array_addresses[0], false),
            AccountMeta::new(tick_array_addresses[1], false),
            AccountMeta::new(tick_array_addresses[2], false),
            AccountMeta::new_readonly(oracle_pda, false),
        ];

        // --- LA CORRECTION EST ICI ---
        // On ajoute le Clock sysvar, qui est attendu par l'instruction de swap
        // pour valider le timestamp.
        accounts.push(AccountMeta::new_readonly(solana_sdk::sysvar::clock::ID, false));

        Ok(Instruction {
            program_id: self.program_id,
            accounts,
            data: instruction_data,
        })
    }
}