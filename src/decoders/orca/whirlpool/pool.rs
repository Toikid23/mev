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
use tokio::runtime::Runtime;
use crate::config::Config;
use async_trait::async_trait;
use crate::decoders::pool_operations::{PoolOperations, UserSwapAccounts};
use solana_sdk::instruction::{Instruction, AccountMeta};
use spl_associated_token_account::get_associated_token_address;
use num_integer::Integer;
use crate::decoders::orca::whirlpool::math::U256;

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
        last_swap_timestamp: 0,
    })
}

// --- IMPLÉMENTATION DE LA FONCTION D'HYDRATATION (NOUVEAU) ---
pub async fn hydrate(pool: &mut DecodedWhirlpoolPool, rpc_client: &RpcClient) -> Result<()> {
    // --- Étape 1: Hydrater les mints (logique inchangée et correcte) ---
    let mints_to_fetch = [pool.mint_a, pool.mint_b];
    let mint_accounts = rpc_client.get_multiple_accounts(&mints_to_fetch).await?;

    let mint_a_account = mint_accounts[0].as_ref().ok_or_else(|| anyhow!("Mint A not found"))?;
    pool.mint_a_program = mint_a_account.owner;
    let decoded_mint_a = spl_token_decoders::mint::decode_mint(&pool.mint_a, &mint_a_account.data)?;
    pool.mint_a_decimals = decoded_mint_a.decimals;
    pool.mint_a_transfer_fee_bps = decoded_mint_a.transfer_fee_basis_points;

    let mint_b_account = mint_accounts[1].as_ref().ok_or_else(|| anyhow!("Mint B not found"))?;
    pool.mint_b_program = mint_b_account.owner;
    let decoded_mint_b = spl_token_decoders::mint::decode_mint(&pool.mint_b, &mint_b_account.data)?;
    pool.mint_b_decimals = decoded_mint_b.decimals;
    pool.mint_b_transfer_fee_bps = decoded_mint_b.transfer_fee_basis_points;

    // --- Étape 2: Charger UNIQUEMENT le TickArray actuel ---
    // On ne charge plus une fenêtre statique, juste le strict nécessaire.
    let mut hydrated_tick_arrays = BTreeMap::new();
    let current_array_start_index = tick_array::get_start_tick_index(pool.tick_current_index, pool.tick_spacing);
    let current_array_address = tick_array::get_tick_array_address(&pool.address, current_array_start_index, &pool.program_id);

    if let Ok(account) = rpc_client.get_account(&current_array_address).await {
        if let Ok(decoded_array) = tick_array::decode_tick_array(&account.data) {
            hydrated_tick_arrays.insert(decoded_array.start_tick_index, decoded_array);
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



impl DecodedWhirlpoolPool {
    // Fonctions utilitaires
    pub fn fee_as_percent(&self) -> f64 { self.fee_rate as f64 / 10_000.0 }
    pub fn get_mints(&self) -> (Pubkey, Pubkey) { (self.mint_a, self.mint_b) }
    pub fn get_vaults(&self) -> (Pubkey, Pubkey) { (self.vault_a, self.vault_b) }

    pub async fn get_quote_with_rpc(
        &mut self,
        token_in_mint: &Pubkey,
        amount_in: u64,
        rpc_client: &RpcClient,
    ) -> Result<u64> {
        println!("\n--- get_quote_async (Whirlpool) DEBUG ---");
        println!("  - Initial amount_in: {}", amount_in);

        let tick_arrays = self.tick_arrays.as_mut().ok_or_else(|| anyhow!("Pool is not hydrated."))?;
        let a_to_b = *token_in_mint == self.mint_a;
        let (in_mint_fee_bps, out_mint_fee_bps) = if a_to_b {
            (self.mint_a_transfer_fee_bps, self.mint_b_transfer_fee_bps)
        } else {
            (self.mint_b_transfer_fee_bps, self.mint_a_transfer_fee_bps)
        };

        let fee_on_input = (amount_in as u128 * in_mint_fee_bps as u128) / 10000;
        let amount_in_after_fee = amount_in.saturating_sub(fee_on_input as u64);
        println!("  - amount_in after input transfer fee: {}", amount_in_after_fee);

        let pool_fee = (amount_in_after_fee as u128 * self.fee_rate as u128) / 1_000_000;
        let mut amount_remaining = amount_in_after_fee.saturating_sub(pool_fee as u64) as u128;
        println!("  - Pool fee: {}. Net amount_remaining to swap: {}", pool_fee, amount_remaining);

        let mut total_amount_out: u128 = 0;
        let mut current_liquidity = self.liquidity;
        let mut current_sqrt_price = self.sqrt_price;
        let mut current_tick_index = self.tick_current_index;
        let mut step = 0;

        let mut fetched_array_indices = std::collections::HashSet::new();
        let current_start_index = tick_array::get_start_tick_index(current_tick_index, self.tick_spacing);
        fetched_array_indices.insert(current_start_index);

        while amount_remaining > 0 && current_liquidity > 0 {
            step += 1;
            println!("\n  [Step {}]", step);
            println!("    - current_sqrt_price: {}", current_sqrt_price);
            println!("    - current_tick_index: {}", current_tick_index);
            println!("    - current_liquidity: {}", current_liquidity);

            let (target_sqrt_price, next_liquidity_net) = {
                let find_result = {
                    let tick_arrays_ref = self.tick_arrays.as_ref().unwrap();
                    find_next_initialized_tick(self, current_tick_index, a_to_b, tick_arrays_ref)
                };
                match find_result {
                    Some((tick_index, tick_data)) => {
                        println!("    - Found next initialized tick at index: {}", tick_index);
                        (orca_whirlpool_math::tick_to_sqrt_price_x64(tick_index), Some(tick_data.liquidity_net))
                    },
                    None => {
                        (if a_to_b { 0 } else { u128::MAX }, None)
                    }
                }
            };

            let (amount_in_step, amount_out_step, next_sqrt_price) = orca_whirlpool_math::compute_swap_step(
                amount_remaining, current_sqrt_price, target_sqrt_price, current_liquidity, a_to_b,
            );
            println!("    - target_sqrt_price: {}", target_sqrt_price);
            println!("    - Consumed in step: {}, Produced in step: {}", amount_in_step, amount_out_step);
            println!("    - next_sqrt_price: {}", next_sqrt_price);

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
        println!("\n  - Total gross amount_out: {}", total_amount_out);
        println!("  - Fee on output: {}", fee_on_output);
        println!("  - Final amount_out: {}", final_amount_out);
        println!("-------------------------------------------\n");

        Ok(final_amount_out as u64)
    }
}

#[async_trait]
impl PoolOperations for DecodedWhirlpoolPool {
    fn get_mints(&self) -> (Pubkey, Pubkey) {
        (self.mint_a, self.mint_b)
    }

    fn get_vaults(&self) -> (Pubkey, Pubkey) {
        (self.vault_a, self.vault_b)
    }

    fn address(&self) -> Pubkey { self.address }

    /// La version synchrone pour Whirlpool retourne une erreur car elle est intrinsèquement
    /// imprécise sans appels RPC.
    fn get_quote(&self, _token_in_mint: &Pubkey, _amount_in: u64, _current_timestamp: i64) -> Result<u64> {
        Err(anyhow!("get_quote synchrone n'est pas supporté pour Whirlpool. Utilisez get_quote_async."))
    }


    fn get_required_input(
        &self,
        token_out_mint: &Pubkey,
        amount_out: u64,
        _current_timestamp: i64,
    ) -> Result<u64> {
        if amount_out == 0 { return Ok(0); }
        let tick_arrays = self.tick_arrays.as_ref().ok_or_else(|| anyhow!("Pool not hydrated."))?;
        if self.liquidity == 0 { return Err(anyhow!("Not enough liquidity in pool.")); }

        let a_to_b = *token_out_mint == self.mint_b; // true = we want B, so we provide A
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

        // La direction du swap pour l'input est l'opposé de la direction de l'output.
        let input_a_to_b = !a_to_b;

        while gross_amount_out_target > 0 {
            if current_liquidity == 0 { return Err(anyhow!("Not enough liquidity to reach target amount out.")); }

            let (next_tick_index, next_liquidity_net) = match find_next_initialized_tick(self, current_tick_index, input_a_to_b, tick_arrays) {
                Some((tick_index, tick_data)) => (tick_index, tick_data.liquidity_net),
                None => (if input_a_to_b { -443636 } else { 443636 }, 0)
            };
            let sqrt_price_target = orca_whirlpool_math::tick_to_sqrt_price_x64(next_tick_index);

            let amount_out_available_in_step = if a_to_b { // we want B
                orca_whirlpool_math::get_delta_y(sqrt_price_target, current_sqrt_price, current_liquidity)
            } else { // we want A
                orca_whirlpool_math::get_delta_x(current_sqrt_price, sqrt_price_target, current_liquidity)
            };

            let amount_out_chunk = gross_amount_out_target.min(amount_out_available_in_step);

            if amount_out_chunk == 0 && gross_amount_out_target > 0 {
                return Err(anyhow!("Calculation stuck, cannot obtain remaining output."));
            }

            let (prev_sqrt_price, amount_in_step_net) = if a_to_b { // we want B (output), so we provide A (input)
                let p_start = orca_whirlpool_math::get_next_sqrt_price_y_down(current_sqrt_price, current_liquidity, amount_out_chunk);
                (p_start, orca_whirlpool_math::get_delta_x_ceil(p_start, current_sqrt_price, current_liquidity))
            } else { // we want A (output), so we provide B (input)
                let p_start = orca_whirlpool_math::get_next_sqrt_price_x_up(current_sqrt_price, current_liquidity, amount_out_chunk);
                (p_start, orca_whirlpool_math::get_delta_y_ceil(current_sqrt_price, p_start, current_liquidity))
            };

            total_amount_in_net += amount_in_step_net;
            gross_amount_out_target -= amount_out_chunk;
            current_sqrt_price = prev_sqrt_price;

            if current_sqrt_price == sqrt_price_target && next_liquidity_net != 0 {
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

        let final_amount_in = if in_mint_fee_bps > 0 {
            let numerator = amount_in_after_transfer_fee.saturating_mul(BPS_DENOMINATOR);
            let denominator = BPS_DENOMINATOR.saturating_sub(in_mint_fee_bps as u128);
            numerator.div_ceil(denominator)
        } else {
            amount_in_after_transfer_fee
        };

        Ok(final_amount_in as u64)
    }



    /// La version asynchrone est la méthode correcte pour obtenir un quote de Whirlpool.
    async fn get_quote_async(&mut self, token_in_mint: &Pubkey, amount_in: u64, rpc_client: &RpcClient) -> Result<u64> {
        // On délègue simplement à la fonction que nous avons déjà écrite.
        self.get_quote_with_rpc(token_in_mint, amount_in, rpc_client).await
    }

    fn create_swap_instruction(
        &self,
        token_in_mint: &Pubkey,
        amount_in: u64,
        min_amount_out: u64,
        user_accounts: &UserSwapAccounts,
    ) -> Result<Instruction> {
        let a_to_b = *token_in_mint == self.mint_a;

        // --- Début de la logique de construction (tirée de votre test.rs) ---

        // 1. Calculer les adresses des 3 tick_arrays
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

        // 2. Déterminer la limite de prix
        let sqrt_price_limit: u128 = if a_to_b { 4295048016 } else { 79226673515401279992447579055 };

        // 3. Construire les données de l'instruction
        let swap_discriminator: [u8; 8] = [248, 198, 158, 145, 225, 117, 135, 200];
        let mut instruction_data = Vec::new();
        instruction_data.extend_from_slice(&swap_discriminator);
        instruction_data.extend_from_slice(&amount_in.to_le_bytes());
        instruction_data.extend_from_slice(&min_amount_out.to_le_bytes());
        instruction_data.extend_from_slice(&sqrt_price_limit.to_le_bytes());
        instruction_data.push(u8::from(true)); // amount_specified_is_input
        instruction_data.push(u8::from(a_to_b));

        // 4. Trouver le PDA de l'oracle
        let (oracle_pda, _) = Pubkey::find_program_address(&[b"oracle", self.address.as_ref()], &self.program_id);

        // 5. Construire la liste des comptes
        let user_ata_for_token_a = get_associated_token_address(&user_accounts.owner, &self.mint_a);
        let user_ata_for_token_b = get_associated_token_address(&user_accounts.owner, &self.mint_b);

        let accounts = vec![
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

        Ok(Instruction {
            program_id: self.program_id,
            accounts,
            data: instruction_data,
        })
    }
}