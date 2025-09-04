// src/decoders/raydium/clmm/pool.rs

use crate::decoders::spl_token_decoders;
use anyhow::{anyhow, bail, Result};
use bytemuck::{from_bytes, Pod, Zeroable};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::instruction::{Instruction, AccountMeta};
use spl_associated_token_account::get_associated_token_address;
use solana_program_pack::Pack;
use std::collections::{BTreeMap, HashSet};
use crate::decoders::raydium::clmm::tick_array::TICK_ARRAY_SIZE;
use super::math;
use super::tick_array::{self, TickArrayState};
use super::tickarray_bitmap_extension;
use super::config;
use serde::{Serialize, Deserialize};
use async_trait::async_trait;
use crate::decoders::pool_operations::{PoolOperations, UserSwapAccounts};
use num_integer::Integer;
use crate::decoders::raydium::clmm::full_math::MulDiv;
use crate::decoders::spl_token_decoders::mint::{calculate_transfer_fee, calculate_gross_amount_before_transfer_fee};
use crate::decoders::orca::whirlpool::math::U256;

// --- STRUCTURES (Inchangées) ---
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecodedClmmPool {
    pub address: Pubkey,
    pub program_id: Pubkey,
    pub amm_config: Pubkey,
    pub observation_key: Pubkey,
    pub mint_a: Pubkey,
    pub mint_b: Pubkey,
    pub mint_a_decimals: u8,
    pub mint_b_decimals: u8,
    pub mint_a_transfer_fee_bps: u16,
    pub mint_b_transfer_fee_bps: u16,
    pub mint_a_max_transfer_fee: u64,
    pub mint_b_max_transfer_fee: u64,
    pub vault_a: Pubkey,
    pub vault_b: Pubkey,
    pub liquidity: u128,
    pub sqrt_price_x64: u128,
    pub tick_current: i32,
    pub tick_spacing: u16,
    pub trade_fee_rate: u32,
    pub min_tick: i32,
    pub max_tick: i32,
    pub tick_arrays: Option<BTreeMap<i32, TickArrayState>>,
    pub last_swap_timestamp: i64,
}

impl DecodedClmmPool {
    pub fn fee_as_percent(&self) -> f64 {
        self.trade_fee_rate as f64 / 1_000_000.0 * 100.0
    }

    // Cette fonction est la version "aller" correcte.
    fn calculate_swap_quote_internal(&self, amount_in: u64, is_base_input: bool) -> Result<(u64, Vec<Pubkey>)> {
        let tick_arrays = self.tick_arrays.as_ref().ok_or_else(|| anyhow!("Pool is not hydrated."))?;
        if tick_arrays.is_empty() || self.liquidity == 0 {
            return Ok((0, Vec::new()));
        }

        const FEE_RATE_DENOMINATOR_VALUE: u64 = 1_000_000;
        let fee_amount = (amount_in as u64).mul_div_ceil(
            self.trade_fee_rate as u64,
            FEE_RATE_DENOMINATOR_VALUE
        ).ok_or_else(|| anyhow!("Math overflow"))?;

        let mut amount_remaining = amount_in.saturating_sub(fee_amount as u64) as u128;

        let mut total_amount_out: u128 = 0;
        let mut current_sqrt_price = self.sqrt_price_x64;
        let mut current_tick_index = self.tick_current;
        let mut current_liquidity = self.liquidity;
        let mut tick_arrays_crossed = HashSet::new();

        while amount_remaining > 0 {
            let current_tick_array_start_index = tick_array::get_start_tick_index(current_tick_index, self.tick_spacing);
            tick_arrays_crossed.insert(tick_array::get_tick_array_address(&self.address, current_tick_array_start_index, &self.program_id));

            if current_liquidity > 0 {
                let (next_tick_index, next_liquidity_net) = match find_next_initialized_tick(self, current_tick_index, is_base_input, tick_arrays) {
                    Ok(result) => result,
                    Err(_) => (if is_base_input { math::MIN_TICK } else { math::MAX_TICK }, 0)
                };
                let sqrt_price_target = math::tick_to_sqrt_price_x64(next_tick_index);

                let (next_sqrt_price, amount_in_step, amount_out_step) = compute_swap_step_without_fee(
                    current_sqrt_price, sqrt_price_target, current_liquidity, amount_remaining, is_base_input,
                )?;

                if amount_in_step == 0 { break; }

                amount_remaining = amount_remaining.saturating_sub(amount_in_step);
                total_amount_out = total_amount_out.saturating_add(amount_out_step);
                current_sqrt_price = next_sqrt_price;

                if current_sqrt_price == sqrt_price_target {
                    current_liquidity = (current_liquidity as i128 + next_liquidity_net) as u128;
                    current_tick_index = if is_base_input { next_tick_index - 1 } else { next_tick_index };
                } else {
                    current_tick_index = math::get_tick_at_sqrt_price(current_sqrt_price)?;
                }
            } else {
                break;
            }
        }

        Ok((total_amount_out as u64, tick_arrays_crossed.into_iter().collect()))
    }

    /// Trouve les N prochains TickArrays initialisés dans la direction du swap.
    fn get_next_initialized_tick_arrays(&self, is_base_input: bool, count: usize) -> Vec<Pubkey> {
        let tick_arrays = self.tick_arrays.as_ref().unwrap();
        let mut result = Vec::new();

        if is_base_input { // Le prix baisse, on cherche des index plus petits
            for (_, array_state) in tick_arrays.iter().rev() {
                if array_state.start_tick_index <= self.tick_current {
                    result.push(tick_array::get_tick_array_address(
                        &self.address,
                        array_state.start_tick_index,
                        &self.program_id,
                    ));
                    if result.len() >= count {
                        break;
                    }
                }
            }
        } else { // Le prix monte, on cherche des index plus grands
            for (_, array_state) in tick_arrays.iter() {
                if array_state.start_tick_index >= self.tick_current {
                    result.push(tick_array::get_tick_array_address(
                        &self.address,
                        array_state.start_tick_index,
                        &self.program_id,
                    ));
                    if result.len() >= count {
                        break;
                    }
                }
            }
        }
        result
    }
}

// ... Le reste du fichier (structs PoolState, RewardInfo, decode_pool, hydrate) ne change pas ...
#[repr(C, packed)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
struct PoolState {
    pub bump: [u8; 1],
    pub amm_config: Pubkey,
    pub owner: Pubkey,
    pub token_mint_0: Pubkey,
    pub token_mint_1: Pubkey,
    pub token_vault_0: Pubkey,
    pub token_vault_1: Pubkey,
    pub observation_key: Pubkey,
    pub mint_decimals_0: u8,
    pub mint_decimals_1: u8,
    pub tick_spacing: u16,
    pub liquidity: u128,
    pub sqrt_price_x64: u128,
    pub tick_current: i32,
    pub padding3: u16,
    pub padding4: u16,
    pub fee_growth_global_0_x64: u128,
    pub fee_growth_global_1_x64: u128,
    pub protocol_fees_token_0: u64,
    pub protocol_fees_token_1: u64,
    pub swap_in_amount_token_0: u128,
    pub swap_out_amount_token_1: u128,
    pub swap_in_amount_token_1: u128,
    pub swap_out_amount_token_0: u128,
    pub status: u8,
    pub padding: [u8; 7],
    pub reward_infos: [RewardInfo; 3],
    pub tick_array_bitmap: [u64; 16], // Le champ qui nous intéresse
    pub total_fees_token_0: u64,
    pub total_fees_claimed_token_0: u64,
    pub total_fees_token_1: u64,
    pub total_fees_claimed_token_1: u64,
    pub fund_fees_token_0: u64,
    pub fund_fees_token_1: u64,
    pub open_time: u64,
    pub recent_epoch: u64,
    pub padding1: [u64; 24],
    pub padding2: [u64; 32],
}
#[repr(C, packed)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
struct RewardInfo {
    pub reward_state: u8,
    pub open_time: u64,
    pub end_time: u64,
    pub last_update_time: u64,
    pub emissions_per_second_x64: u128,
    pub reward_total_emissioned: u64,
    pub reward_claimed: u64,
    pub token_mint: Pubkey,
    pub token_vault: Pubkey,
    pub authority: Pubkey,
    pub reward_growth_global_x64: u128,
}
pub fn decode_pool(address: &Pubkey, data: &[u8], program_id: &Pubkey) -> Result<DecodedClmmPool> {
    const DISCRIMINATOR: [u8; 8] = [247, 237, 227, 245, 215, 195, 222, 70];
    if data.get(..8) != Some(&DISCRIMINATOR) { bail!("Invalid PoolState discriminator."); }
    let data_slice = &data[8..];
    if data_slice.len() < std::mem::size_of::<PoolState>() { bail!("PoolState data length mismatch."); }
    let pool_struct: &PoolState = from_bytes(&data_slice[..std::mem::size_of::<PoolState>()]);

    Ok(DecodedClmmPool {
        address: *address,
        program_id: *program_id,
        amm_config: pool_struct.amm_config,
        observation_key: pool_struct.observation_key,
        mint_a: pool_struct.token_mint_0,
        mint_b: pool_struct.token_mint_1,
        vault_a: pool_struct.token_vault_0,
        vault_b: pool_struct.token_vault_1,
        tick_spacing: pool_struct.tick_spacing,
        liquidity: pool_struct.liquidity,
        sqrt_price_x64: pool_struct.sqrt_price_x64,
        tick_current: pool_struct.tick_current,
        mint_a_decimals: 0, mint_b_decimals: 0,
        mint_a_transfer_fee_bps: 0, mint_b_transfer_fee_bps: 0,
        mint_a_max_transfer_fee: 0,
        mint_b_max_transfer_fee: 0,
        trade_fee_rate: 0, min_tick: -443636, max_tick: 443636,
        tick_arrays: None,
        last_swap_timestamp: 0,
    })
}
pub async fn hydrate(pool: &mut DecodedClmmPool, rpc_client: &RpcClient) -> Result<()> {
    let bitmap_ext_address = tickarray_bitmap_extension::get_bitmap_extension_address(&pool.address, &pool.program_id);

    let (config_res, mint_a_res, mint_b_res, pool_state_res, bitmap_ext_res) = tokio::join!(
        rpc_client.get_account_data(&pool.amm_config),
        rpc_client.get_account_data(&pool.mint_a),
        rpc_client.get_account_data(&pool.mint_b),
        rpc_client.get_account_data(&pool.address),
        rpc_client.get_account(&bitmap_ext_address)
    );

    let config_account_data = config_res?;
    let decoded_config = config::decode_config(&config_account_data)?;
    pool.tick_spacing = decoded_config.tick_spacing;
    pool.trade_fee_rate = decoded_config.trade_fee_rate;

    let mint_a_data = mint_a_res?;
    let decoded_mint_a = spl_token_decoders::mint::decode_mint(&pool.mint_a, &mint_a_data)?;
    pool.mint_a_decimals = decoded_mint_a.decimals;
    pool.mint_a_transfer_fee_bps = decoded_mint_a.transfer_fee_basis_points;
    pool.mint_a_max_transfer_fee = decoded_mint_a.max_transfer_fee;

    let mint_b_data = mint_b_res?;
    let decoded_mint_b = spl_token_decoders::mint::decode_mint(&pool.mint_b, &mint_b_data)?;
    pool.mint_b_decimals = decoded_mint_b.decimals;
    pool.mint_b_transfer_fee_bps = decoded_mint_b.transfer_fee_basis_points;
    pool.mint_b_max_transfer_fee = decoded_mint_b.max_transfer_fee;

    let pool_state_data = pool_state_res?;
    const POOL_STATE_DISCRIMINATOR: [u8; 8] = [247, 237, 227, 245, 215, 195, 222, 70];
    if pool_state_data.get(..8) != Some(&POOL_STATE_DISCRIMINATOR) { bail!("Invalid PoolState discriminator during hydration."); }
    let data_slice = &pool_state_data[8..];
    if data_slice.len() < std::mem::size_of::<PoolState>() { bail!("PoolState data is too short."); }
    let pool_state_struct: &PoolState = from_bytes(&data_slice[..std::mem::size_of::<PoolState>()]);

    let default_bitmap = pool_state_struct.tick_array_bitmap;
    let extension_bitmap_words = if let Ok(account) = bitmap_ext_res {
        tickarray_bitmap_extension::decode_tick_array_bitmap_extension(&account.data)?.bitmap_words
    } else {
        Vec::new()
    };

    let mut addresses_to_fetch = HashSet::new();
    let multiplier = (tick_array::TICK_ARRAY_SIZE as i32) * (pool.tick_spacing as i32);
    let ticks_in_one_bitmap = 512 * multiplier;

    for (word_index, &word) in default_bitmap.iter().enumerate() {
        if word == 0 { continue; }
        for bit_index in 0..64 {
            if (word & (1 << bit_index)) != 0 {
                let compressed_index = word_index * 64 + bit_index;
                let start_tick_index = (compressed_index as i32 - 512) * multiplier;
                if start_tick_index >= math::MIN_TICK && start_tick_index <= math::MAX_TICK {
                    addresses_to_fetch.insert(tick_array::get_tick_array_address(&pool.address, start_tick_index, &pool.program_id));
                }
            }
        }
    }

    let negative_pages_len = 14 * 8;
    for (word_index_flat, &word) in extension_bitmap_words.iter().enumerate() {
        if word == 0 { continue; }
        for bit_index in 0..64 {
            if (word & (1 << bit_index)) != 0 {
                let start_tick_index = if word_index_flat < negative_pages_len {
                    let page_index = word_index_flat / 8;
                    let bit_pos_in_page = (word_index_flat % 8) * 64 + bit_index;
                    -(ticks_in_one_bitmap * (page_index as i32 + 1)) + (bit_pos_in_page as i32 * multiplier)
                } else {
                    let pos_idx = word_index_flat - negative_pages_len;
                    let page_index = pos_idx / 8;
                    let bit_pos_in_page = (pos_idx % 8) * 64 + bit_index;
                    ticks_in_one_bitmap * (page_index as i32 + 1) + (bit_pos_in_page as i32 * multiplier)
                };

                if start_tick_index >= math::MIN_TICK && start_tick_index <= math::MAX_TICK {
                    addresses_to_fetch.insert(tick_array::get_tick_array_address(&pool.address, start_tick_index, &pool.program_id));
                }
            }
        }
    }

    if addresses_to_fetch.is_empty() {
        pool.tick_arrays = Some(BTreeMap::new());
        return Ok(());
    }

    let accounts_results = rpc_client.get_multiple_accounts(&addresses_to_fetch.into_iter().collect::<Vec<_>>()).await?;
    let mut tick_arrays = BTreeMap::new();
    for account_opt in accounts_results {
        if let Some(account) = account_opt {
            if let Ok(decoded_array) = tick_array::decode_tick_array(&account.data) {
                tick_arrays.insert(decoded_array.start_tick_index, decoded_array);
            }
        }
    }
    pool.tick_arrays = Some(tick_arrays);
    Ok(())
}


#[async_trait]
impl PoolOperations for DecodedClmmPool {
    fn get_mints(&self) -> (Pubkey, Pubkey) {
        (self.mint_a, self.mint_b)
    }

    fn get_vaults(&self) -> (Pubkey, Pubkey) {
        (self.vault_a, self.vault_b)
    }

    fn get_reserves(&self) -> (u64, u64) {
        if let Some(tick_arrays) = &self.tick_arrays {
            let mut total_liquidity: u128 = 0;
            // On somme la liquidité brute de tous les ticks initialisés dans tous les arrays chargés.
            for (_, tick_array_state) in tick_arrays.iter() {
                for tick in tick_array_state.ticks.iter() {
                    total_liquidity = total_liquidity.saturating_add(tick.liquidity_gross);
                }
            }

            // On estime les réserves totales en se basant sur la liquidité totale et le prix actuel.
            // C'est une approximation, mais elle est basée sur l'ensemble de la liquidité du pool.
            let price_x64 = U256::from(self.sqrt_price_x64);
            let liquidity_u256 = U256::from(total_liquidity);
            let q64 = U256::one() << 64;

            if price_x64.is_zero() { return (0, 0); }

            let estimated_reserve_a = (liquidity_u256 * q64 / price_x64).as_u64();
            let estimated_reserve_b = ((liquidity_u256 * price_x64) >> 64).as_u64();

            (estimated_reserve_a, estimated_reserve_b)
        } else {
            // Si le pool n'a pas été hydraté, on ne peut rien dire.
            (0, 0)
        }
    }

    fn address(&self) -> Pubkey { self.address }

    fn get_quote(&self, token_in_mint: &Pubkey, amount_in: u64, _current_timestamp: i64) -> Result<u64> {
        let is_base_input = *token_in_mint == self.mint_a;
        let (in_mint_fee_bps, in_mint_max_fee, out_mint_fee_bps, out_mint_max_fee) = if is_base_input {
            (self.mint_a_transfer_fee_bps, self.mint_a_max_transfer_fee, self.mint_b_transfer_fee_bps, self.mint_b_max_transfer_fee)
        } else {
            (self.mint_b_transfer_fee_bps, self.mint_b_max_transfer_fee, self.mint_a_transfer_fee_bps, self.mint_a_max_transfer_fee)
        };

        let fee_on_input = calculate_transfer_fee(amount_in, in_mint_fee_bps, in_mint_max_fee)?;
        let net_amount_in = amount_in.saturating_sub(fee_on_input);

        let (gross_amount_out, _) = self.calculate_swap_quote_internal(net_amount_in, is_base_input)?;
        let fee_on_output = calculate_transfer_fee(gross_amount_out, out_mint_fee_bps, out_mint_max_fee)?;

        Ok(gross_amount_out.saturating_sub(fee_on_output))
    }


    fn get_required_input(&mut self, token_out_mint: &Pubkey, amount_out: u64, _current_timestamp: i64) -> Result<u64> {
        if amount_out == 0 { return Ok(0); }
        let tick_arrays = self.tick_arrays.as_ref().ok_or_else(|| anyhow!("Pool not hydrated."))?;
        if tick_arrays.is_empty() || self.liquidity == 0 {
            return Err(anyhow!("Not enough liquidity in pool (no tick arrays)."));
        }

        let is_base_output = *token_out_mint == self.mint_a;
        let is_base_input = !is_base_output;

        let (in_mint_fee_bps, in_mint_max_fee, out_mint_fee_bps, out_mint_max_fee) = if is_base_input {
            (self.mint_a_transfer_fee_bps, self.mint_a_max_transfer_fee, self.mint_b_transfer_fee_bps, self.mint_b_max_transfer_fee)
        } else {
            (self.mint_b_transfer_fee_bps, self.mint_b_max_transfer_fee, self.mint_a_transfer_fee_bps, self.mint_a_max_transfer_fee)
        };

        let gross_amount_out_target = calculate_gross_amount_before_transfer_fee(amount_out, out_mint_fee_bps, out_mint_max_fee)? as u128;

        // --- LA CORRECTION EST ICI ---
        let mut mutable_gross_amount_out_target = gross_amount_out_target;

        let mut total_amount_in_net: u128 = 0;
        let mut current_sqrt_price = self.sqrt_price_x64;
        let mut current_tick_index = self.tick_current;
        let mut current_liquidity = self.liquidity;

        while mutable_gross_amount_out_target > 0 {
            if current_liquidity == 0 { return Err(anyhow!("Not enough liquidity to reach target amount out.")); }

            let (next_tick_index, next_liquidity_net) = match find_next_initialized_tick(self, current_tick_index, is_base_input, tick_arrays) {
                Ok(result) => result,
                Err(_) => (if is_base_input { math::MIN_TICK } else { math::MAX_TICK }, 0)
            };
            let sqrt_price_target = math::tick_to_sqrt_price_x64(next_tick_index);

            let amount_out_available_in_step = (if is_base_output {
                math::get_amount_x(sqrt_price_target, current_sqrt_price, current_liquidity, false)?
            } else {
                math::get_amount_y(current_sqrt_price, sqrt_price_target, current_liquidity, false)?
            }) as u128;

            let amount_out_chunk = mutable_gross_amount_out_target.min(amount_out_available_in_step);
            if amount_out_chunk == 0 && mutable_gross_amount_out_target > 0 { break; }

            let (prev_sqrt_price, amount_in_step_net) = if is_base_output {
                let starting_sqrt_price = math::get_sqrt_price_from_amount_x_out(current_sqrt_price, current_liquidity, amount_out_chunk);
                let required_y = math::get_amount_y(starting_sqrt_price, current_sqrt_price, current_liquidity, true)?;
                (starting_sqrt_price, required_y as u128)
            } else {
                let starting_sqrt_price = math::get_sqrt_price_from_amount_y_out(current_sqrt_price, current_liquidity, amount_out_chunk);
                let required_x = math::get_amount_x(current_sqrt_price, starting_sqrt_price, current_liquidity, true)?;
                (starting_sqrt_price, required_x as u128)
            };

            total_amount_in_net = total_amount_in_net.saturating_add(amount_in_step_net);
            mutable_gross_amount_out_target = mutable_gross_amount_out_target.saturating_sub(amount_out_chunk);
            current_sqrt_price = prev_sqrt_price;

            if current_sqrt_price == sqrt_price_target && next_liquidity_net != 0 {
                current_liquidity = (current_liquidity as i128 + next_liquidity_net) as u128;
                current_tick_index = if is_base_input { next_tick_index - 1 } else { next_tick_index };
            } else {
                current_tick_index = math::get_tick_at_sqrt_price(current_sqrt_price)?;
            }
        }

        const FEE_RATE_DENOMINATOR_VALUE: u64 = 1_000_000;
        let amount_in_after_transfer_fee = (total_amount_in_net as u64).mul_div_ceil(
            FEE_RATE_DENOMINATOR_VALUE,
            FEE_RATE_DENOMINATOR_VALUE - self.trade_fee_rate as u64
        ).ok_or_else(|| anyhow!("Math overflow"))?;

        calculate_gross_amount_before_transfer_fee(amount_in_after_transfer_fee, in_mint_fee_bps, in_mint_max_fee)
    }


    fn create_swap_instruction(
        &self,
        token_in_mint: &Pubkey,
        amount_in: u64,
        min_amount_out: u64,
        user_accounts: &UserSwapAccounts,
    ) -> Result<Instruction> {
        let is_base_input = *token_in_mint == self.mint_a;
        let mut tick_arrays = self.get_next_initialized_tick_arrays(is_base_input, 3);
        if tick_arrays.is_empty() {
            bail!("Impossible de trouver des tick_arrays initialisés pour le swap CLMM.");
        }
        while tick_arrays.len() < 3 {
            tick_arrays.push(*tick_arrays.last().unwrap());
        }

        let sqrt_price_limit = if is_base_input {
            4295128739_u128
        } else {
            79226673515401279992447579055_u128
        };

        let mut instruction_data: Vec<u8> = vec![43, 4, 237, 11, 26, 201, 30, 98];
        instruction_data.extend_from_slice(&amount_in.to_le_bytes());
        instruction_data.extend_from_slice(&min_amount_out.to_le_bytes());
        instruction_data.extend_from_slice(&sqrt_price_limit.to_le_bytes());
        instruction_data.push(u8::from(is_base_input));

        let (input_vault, output_vault, input_vault_mint, output_vault_mint) = if is_base_input {
            (self.vault_a, self.vault_b, self.mint_a, self.mint_b)
        } else {
            (self.vault_b, self.vault_a, self.mint_b, self.mint_a)
        };

        let mut accounts = vec![
            AccountMeta { pubkey: user_accounts.owner, is_signer: true, is_writable: false },
            AccountMeta { pubkey: self.amm_config, is_signer: false, is_writable: false },
            AccountMeta { pubkey: self.address, is_signer: false, is_writable: true },
            AccountMeta { pubkey: user_accounts.source, is_signer: false, is_writable: true },
            AccountMeta { pubkey: user_accounts.destination, is_signer: false, is_writable: true },
            AccountMeta { pubkey: input_vault, is_signer: false, is_writable: true },
            AccountMeta { pubkey: output_vault, is_signer: false, is_writable: true },
            AccountMeta { pubkey: self.observation_key, is_signer: false, is_writable: true },
            AccountMeta { pubkey: spl_token::id(), is_signer: false, is_writable: false },
            AccountMeta { pubkey: spl_token_2022::id(), is_signer: false, is_writable: false },
            AccountMeta { pubkey: spl_memo::id(), is_signer: false, is_writable: false },
            AccountMeta { pubkey: input_vault_mint, is_signer: false, is_writable: false },
            AccountMeta { pubkey: output_vault_mint, is_signer: false, is_writable: false },
        ];

        let tick_array_bitmap_extension = tickarray_bitmap_extension::get_bitmap_extension_address(&self.address, &self.program_id);
        accounts.push(AccountMeta { pubkey: tick_array_bitmap_extension, is_signer: false, is_writable: true });

        for key in tick_arrays {
            accounts.push(AccountMeta { pubkey: key, is_signer: false, is_writable: true });
        }

        Ok(Instruction {
            program_id: self.program_id,
            accounts,
            data: instruction_data,
        })
    }
}

// Nouvelle fonction helper qui ne gère pas les frais
fn compute_swap_step_without_fee(
    sqrt_price_current_x64: u128,
    sqrt_price_target_x64: u128,
    liquidity: u128,
    amount_remaining: u128,
    is_base_input: bool,
) -> Result<(u128, u128, u128)> {
    let mut sqrt_price_next_x64: u128;
    let amount_in: u128;
    let amount_out: u128;

    if is_base_input {
        let amount_in_to_reach_target = math::get_amount_x(sqrt_price_target_x64, sqrt_price_current_x64, liquidity, true)? as u128;
        if amount_remaining >= amount_in_to_reach_target {
            sqrt_price_next_x64 = sqrt_price_target_x64;
            amount_in = amount_in_to_reach_target;
        } else {
            sqrt_price_next_x64 = math::get_next_sqrt_price_from_amount_x_in(sqrt_price_current_x64, liquidity, amount_remaining, true);
            amount_in = amount_remaining;
        }
        amount_out = math::get_amount_y(sqrt_price_next_x64, sqrt_price_current_x64, liquidity, false)? as u128;
    } else {
        let amount_in_to_reach_target = math::get_amount_y(sqrt_price_current_x64, sqrt_price_target_x64, liquidity, true)? as u128;
        if amount_remaining >= amount_in_to_reach_target {
            sqrt_price_next_x64 = sqrt_price_target_x64;
            amount_in = amount_in_to_reach_target;
        } else {
            sqrt_price_next_x64 = math::get_next_sqrt_price_from_amount_y_in(sqrt_price_current_x64, liquidity, amount_remaining, true);
            amount_in = amount_remaining;
        }
        amount_out = math::get_amount_x(sqrt_price_current_x64, sqrt_price_next_x64, liquidity, false)? as u128;
    }
    Ok((sqrt_price_next_x64, amount_in, amount_out))
}


fn find_next_initialized_tick<'a>(
    pool: &'a DecodedClmmPool,
    current_tick: i32,
    is_base_input: bool, // true = zero_for_one (cherche vers le bas)
    tick_arrays: &'a BTreeMap<i32, TickArrayState>,
) -> Result<(i32, i128)> {
    let tick_spacing = pool.tick_spacing as i32;

    if is_base_input {
        let current_array_start_index = tick_array::get_start_tick_index(current_tick, pool.tick_spacing);
        let mut tick_offset_in_array = (current_tick - current_array_start_index) / tick_spacing;

        if let Some(array_state) = tick_arrays.get(&current_array_start_index) {
            while tick_offset_in_array >= 0 {
                let tick_state = &array_state.ticks[tick_offset_in_array as usize];
                if tick_state.liquidity_gross > 0 {
                    return Ok((tick_state.tick, tick_state.liquidity_net));
                }
                tick_offset_in_array -= 1;
            }
        }

        for (_, array_state) in tick_arrays.range(..current_array_start_index).rev() {
            for i in (0..TICK_ARRAY_SIZE).rev() {
                let tick_state = &array_state.ticks[i];
                if tick_state.liquidity_gross > 0 {
                    return Ok((tick_state.tick, tick_state.liquidity_net));
                }
            }
        }
    } else {
        let current_array_start_index = tick_array::get_start_tick_index(current_tick, pool.tick_spacing);
        let mut tick_offset_in_array = ((current_tick - current_array_start_index) / tick_spacing) + 1;

        if let Some(array_state) = tick_arrays.get(&current_array_start_index) {
            while tick_offset_in_array < TICK_ARRAY_SIZE as i32 {
                let tick_state = &array_state.ticks[tick_offset_in_array as usize];
                if tick_state.liquidity_gross > 0 {
                    return Ok((tick_state.tick, tick_state.liquidity_net));
                }
                tick_offset_in_array += 1;
            }
        }

        for (start_index, array_state) in tick_arrays.range(current_array_start_index + 1..) {
            if *start_index > current_array_start_index + (TICK_ARRAY_SIZE as i32 * tick_spacing) { break; }
            for i in 0..TICK_ARRAY_SIZE {
                let tick_state = &array_state.ticks[i];
                if tick_state.liquidity_gross > 0 {
                    return Ok((tick_state.tick, tick_state.liquidity_net));
                }
            }
        }
    }
    Err(anyhow!("Aucun tick initialisé trouvé dans la direction du swap parmi les arrays chargés."))
}