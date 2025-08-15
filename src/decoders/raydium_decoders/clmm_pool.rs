// Fichier : src/decoders/raydium_decoders/clmm_pool.rs
// STATUT : VERSION FINALE ET CORRECTE - Assurez-vous que ce code est bien celui qui est compilé.
use crate::decoders::pool_operations::PoolOperations;
use crate::decoders::raydium_decoders::clmm_config;
use crate::decoders::raydium_decoders::tick_array::{self, TickArrayState};
use crate::decoders::spl_token_decoders;
use crate::math::clmm_math;
use anyhow::{anyhow, bail, Result};
use bytemuck::{from_bytes, Pod, Zeroable};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::pubkey::Pubkey;
use crate::decoders::raydium_decoders::tickarray_bitmap_extension;
// Imports nécessaires pour la construction d'instruction
use solana_sdk::instruction::{Instruction, AccountMeta};
use spl_associated_token_account::get_associated_token_address;
use solana_program_pack::Pack;
use std::collections::{BTreeMap, HashSet};
use crate::decoders::raydium_decoders::tick_array::TICK_ARRAY_SIZE;


// --- STRUCTURES (Inchangées) ---
#[derive(Debug, Clone)]
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
}

impl DecodedClmmPool {
    pub fn fee_as_percent(&self) -> f64 {
        self.trade_fee_rate as f64 / 1_000_000.0 * 100.0
    }

    pub fn get_quote_with_tick_arrays(&self, token_in_mint: &Pubkey, amount_in: u64, _current_timestamp: i64) -> Result<(u64, Vec<Pubkey>)> {
        let tick_arrays = self.tick_arrays.as_ref().ok_or_else(|| anyhow!("Pool is not hydrated."))?;
        if tick_arrays.is_empty() || self.liquidity == 0 {
            return Ok((0, Vec::new()));
        }

        let is_base_input = *token_in_mint == self.mint_a;
        let fee_on_input = (amount_in as u128 * self.mint_a_transfer_fee_bps as u128) / 10000;
        let mut amount_remaining = amount_in.saturating_sub(fee_on_input as u64) as u128;
        let mut total_amount_out: u128 = 0;
        let mut current_sqrt_price = self.sqrt_price_x64;
        let mut current_tick_index = self.tick_current;
        let mut current_liquidity = self.liquidity;
        let mut tick_arrays_crossed = HashSet::new();

        while amount_remaining > 0 {
            let current_tick_array_start_index = tick_array::get_start_tick_index(current_tick_index, self.tick_spacing);
            tick_arrays_crossed.insert(tick_array::get_tick_array_address(&self.address, current_tick_array_start_index, &self.program_id));

            if current_liquidity > 0 {
                let (next_tick_index, _) = match find_next_initialized_tick(self, current_tick_index, is_base_input, tick_arrays) {
                    Ok(result) => result,
                    Err(_) => (if is_base_input { clmm_math::MIN_TICK } else { clmm_math::MAX_TICK }, 0)
                };

                let sqrt_price_target = clmm_math::tick_to_sqrt_price_x64(next_tick_index);

                let (next_sqrt_price, amount_in_step, amount_out_step, fee_amount_step) = clmm_math::compute_swap_step(
                    current_sqrt_price, sqrt_price_target, current_liquidity, amount_remaining, self.trade_fee_rate, is_base_input,
                )?;

                let total_consumed = amount_in_step.saturating_add(fee_amount_step);
                if total_consumed == 0 { break; }

                amount_remaining = amount_remaining.saturating_sub(total_consumed);
                total_amount_out = total_amount_out.saturating_add(amount_out_step);
                current_sqrt_price = next_sqrt_price;
            }

            if current_sqrt_price == clmm_math::tick_to_sqrt_price_x64(find_next_initialized_tick(self, current_tick_index, is_base_input, tick_arrays)?.0) {
                let (next_tick_index, next_liquidity_net) = find_next_initialized_tick(self, current_tick_index, is_base_input, tick_arrays)?;
                current_liquidity = (current_liquidity as i128 + next_liquidity_net) as u128;
                current_tick_index = if is_base_input { next_tick_index - 1 } else { next_tick_index };

                let next_tick_array_start_index = tick_array::get_start_tick_index(next_tick_index, self.tick_spacing);
                tick_arrays_crossed.insert(tick_array::get_tick_array_address(&self.address, next_tick_array_start_index, &self.program_id));
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
            // On itère sur les tick_arrays hydratés en ordre inversé (du plus grand au plus petit start_tick_index)
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
            // On itère sur les tick_arrays hydratés en ordre normal
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

    pub fn create_swap_instruction(
        &self,
        input_token_mint: &Pubkey,
        user_source_token_account: &Pubkey,
        user_destination_token_account: &Pubkey,
        user_owner: &Pubkey,
        amount_in: u64,
        minimum_amount_out: u64,
    ) -> Result<Instruction> {
        let mut instruction_data: Vec<u8> = vec![122, 22, 232, 163, 102, 137, 13, 52]; // swap_v2
        instruction_data.extend_from_slice(&amount_in.to_le_bytes());
        instruction_data.extend_from_slice(&minimum_amount_out.to_le_bytes());
        instruction_data.extend_from_slice(&u128::to_le_bytes(0)); // sqrt_price_limit
        instruction_data.extend_from_slice(&[1]); // is_base_input = true

        let is_base_input = *input_token_mint == self.mint_a;

        let (input_vault, output_vault, input_mint, output_mint) = if is_base_input {
            (self.vault_a, self.vault_b, self.mint_a, self.mint_b)
        } else {
            (self.vault_b, self.vault_a, self.mint_b, self.mint_a)
        };

        let mut accounts = vec![
            AccountMeta::new_readonly(*user_owner, true),
            AccountMeta::new_readonly(self.amm_config, false),
            AccountMeta::new(self.address, false),
            AccountMeta::new(*user_source_token_account, false),
            AccountMeta::new(*user_destination_token_account, false),
            AccountMeta::new(input_vault, false),
            AccountMeta::new(output_vault, false),
            AccountMeta::new(self.observation_key, false),
            AccountMeta::new_readonly(spl_token::id(), false),
            AccountMeta::new_readonly(spl_token_2022::id(), false),
            AccountMeta::new_readonly(spl_memo::id(), false),
            AccountMeta::new_readonly(input_mint, false),
            AccountMeta::new_readonly(output_mint, false),
        ];

        let tick_array_bitmap_extension = tickarray_bitmap_extension::get_bitmap_extension_address(&self.address, &self.program_id);
        accounts.push(AccountMeta::new(tick_array_bitmap_extension, false));

        // On fournit les 3 prochains tick arrays initialisés
        let tick_array_keys = self.get_next_initialized_tick_arrays(is_base_input, 3);
        for key in tick_array_keys {
            accounts.push(AccountMeta::new(key, false));
        }

        Ok(Instruction {
            program_id: self.program_id,
            accounts,
            data: instruction_data,
        })
    }
}

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

// Nous avons aussi besoin de la définition de RewardInfo pour que la taille soit correcte
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

// --- LOGIQUE DE DÉCODAGE ET HYDRATATION ---
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
        trade_fee_rate: 0, min_tick: -443636, max_tick: 443636,
        tick_arrays: None,
    })
}

// REMPLACEZ VOTRE FONCTION HYDRATE PAR CELLE-CI (VERSION CORRIGÉE POUR COMPILER) :
pub async fn hydrate(pool: &mut DecodedClmmPool, rpc_client: &RpcClient) -> Result<()> {
    println!("[HYDRATE DEBUG] Début de l'hydratation pour le pool {}", pool.address);

    let bitmap_ext_address = tickarray_bitmap_extension::get_bitmap_extension_address(&pool.address, &pool.program_id);
    println!("[HYDRATE DEBUG] Adresse du bitmap d'extension calculée : {}", bitmap_ext_address);

    let (config_res, mint_a_res, mint_b_res, pool_state_res, bitmap_ext_res) = tokio::join!(
        rpc_client.get_account_data(&pool.amm_config),
        rpc_client.get_account_data(&pool.mint_a),
        rpc_client.get_account_data(&pool.mint_b),
        rpc_client.get_account_data(&pool.address),
        rpc_client.get_account(&bitmap_ext_address)
    );
    println!("[HYDRATE DEBUG] Appels RPC terminés.");

    let config_account_data = config_res?;
    let decoded_config = clmm_config::decode_config(&config_account_data)?;
    pool.tick_spacing = decoded_config.tick_spacing;
    pool.trade_fee_rate = decoded_config.trade_fee_rate;

    let mint_a_data = mint_a_res?;
    let decoded_mint_a = spl_token_decoders::mint::decode_mint(&pool.mint_a, &mint_a_data)?;
    pool.mint_a_decimals = decoded_mint_a.decimals;
    pool.mint_a_transfer_fee_bps = decoded_mint_a.transfer_fee_basis_points;

    let mint_b_data = mint_b_res?;
    let decoded_mint_b = spl_token_decoders::mint::decode_mint(&pool.mint_b, &mint_b_data)?;
    pool.mint_b_decimals = decoded_mint_b.decimals;
    pool.mint_b_transfer_fee_bps = decoded_mint_b.transfer_fee_basis_points;

    // --- Étape 2: Lecture ROBUSTE des bitmaps ---
    let pool_state_data = pool_state_res?;
    println!("[HYDRATE DEBUG] Taille des données du PoolState reçues : {} octets.", pool_state_data.len());

    const POOL_STATE_DISCRIMINATOR: [u8; 8] = [247, 237, 227, 245, 215, 195, 222, 70];
    if pool_state_data.get(..8) != Some(&POOL_STATE_DISCRIMINATOR) {
        bail!("Invalid PoolState discriminator during hydration.");
    }
    let data_slice = &pool_state_data[8..];
    if data_slice.len() < std::mem::size_of::<PoolState>() {
        bail!("Les données du PoolState sont trop courtes pour être parsées (taille: {}, struct requise: {}).", data_slice.len(), std::mem::size_of::<PoolState>());
    }

    // LA CORRECTION DÉFINITIVE : On parse toute la struct et on accède au champ directement.
    let pool_state_struct: &PoolState = from_bytes(&data_slice[..std::mem::size_of::<PoolState>()]);
    let default_bitmap = pool_state_struct.tick_array_bitmap;
    println!("[HYDRATE DEBUG] Bitmap par défaut lu directement depuis la struct : {:?}", default_bitmap);

    // Le reste de la logique reste le même, car il était déjà correct.
    let extension_bitmap_words = if let Ok(account) = bitmap_ext_res {
        println!("[HYDRATE DEBUG] Compte de bitmap d'extension trouvé !");
        tickarray_bitmap_extension::decode_tick_array_bitmap_extension(&account.data)?.bitmap_words
    } else {
        println!("[HYDRATE DEBUG] Le compte de bitmap d'extension n'a PAS été trouvé.");
        Vec::new()
    };

    // ... (le reste de la fonction est inchangé)
    println!("[HYDRATE DEBUG] Début de la génération des adresses de TickArray...");
    let mut addresses_to_fetch = Vec::new();
    let multiplier = (tick_array::TICK_ARRAY_SIZE as i32) * (pool.tick_spacing as i32);

    for (word_index, &word) in default_bitmap.iter().enumerate() {
        if word == 0 { continue; }
        for bit_index in 0..64 {
            if (word & (1 << bit_index)) != 0 {
                let compressed_index = word_index * 64 + bit_index;
                let start_tick_index = (compressed_index as i32 - 512) * multiplier;
                if start_tick_index >= clmm_math::MIN_TICK && start_tick_index <= clmm_math::MAX_TICK {
                    let address = tick_array::get_tick_array_address(&pool.address, start_tick_index, &pool.program_id);
                    println!("[HYDRATE DEBUG][DEFAULT] Bit trouvé ! word[{}], bit {}. start_tick_index: {}, adresse: {}", word_index, bit_index, start_tick_index, address);
                    addresses_to_fetch.push(address);
                }
            }
        }
    }

    let ticks_in_one_bitmap = multiplier * 512;
    for (word_index, &word) in extension_bitmap_words.iter().enumerate() {
        if word == 0 { continue; }
        for bit_index in 0..64 {
            if (word & (1 << bit_index)) != 0 {
                let compressed_index = word_index * 64 + bit_index;
                let start_tick_index = if compressed_index < (112 * 64) {
                    let tick_offset = (compressed_index / 512) as i32;
                    let bit_pos_in_sub = (compressed_index % 512) as i32;
                    -(ticks_in_one_bitmap * (tick_offset + 1)) + (bit_pos_in_sub * multiplier)
                } else {
                    let pos_idx = compressed_index - (112 * 64);
                    let tick_offset = (pos_idx / 512) as i32;
                    let bit_pos_in_sub = (pos_idx % 512) as i32;
                    ticks_in_one_bitmap * (tick_offset + 1) + (bit_pos_in_sub * multiplier)
                };
                if start_tick_index >= clmm_math::MIN_TICK && start_tick_index <= clmm_math::MAX_TICK {
                    let address = tick_array::get_tick_array_address(&pool.address, start_tick_index, &pool.program_id);
                    println!("[HYDRATE DEBUG][EXTENSION] Bit trouvé ! word[{}], bit {}. start_tick_index: {}, adresse: {}", word_index, bit_index, start_tick_index, address);
                    addresses_to_fetch.push(address);
                }
            }
        }
    }

    println!("[HYDRATE DEBUG] Nombre total d'adresses de TickArray à fetcher : {}", addresses_to_fetch.len());
    if addresses_to_fetch.is_empty() {
        pool.tick_arrays = Some(BTreeMap::new());
        println!("[HYDRATE DEBUG] Aucune adresse de TickArray à fetcher. Hydratation terminée.");
        return Ok(());
    }

    let accounts_results = rpc_client.get_multiple_accounts(&addresses_to_fetch).await?;
    let found_accounts_count = accounts_results.iter().filter(|a| a.is_some()).count();
    println!("[HYDRATE DEBUG] get_multiple_accounts a retourné {} comptes sur les {} demandés.", found_accounts_count, addresses_to_fetch.len());

    let mut tick_arrays = BTreeMap::new();
    for account_opt in accounts_results {
        if let Some(account) = account_opt {
            match tick_array::decode_tick_array(&account.data) {
                Ok(decoded_array) => {
                    let start_tick_index = decoded_array.start_tick_index;
                    println!("[HYDRATE DEBUG] TickArray décodé avec succès pour start_tick_index: {}", start_tick_index);
                    tick_arrays.insert(start_tick_index, decoded_array);
                }
                Err(e) => {
                    println!("[HYDRATE DEBUG] ERREUR lors du décodage d'un TickArray: {}", e);
                }
            }
        }
    }

    let tick_arrays_count = tick_arrays.len();
    pool.tick_arrays = Some(tick_arrays);
    println!("[HYDRATE DEBUG] Hydratation terminée. Trouvé {} TickArray(s) valides.", tick_arrays_count);

    Ok(())
}



impl PoolOperations for DecodedClmmPool {
    fn get_mints(&self) -> (Pubkey, Pubkey) {
        (self.mint_a, self.mint_b)
    }
    fn get_vaults(&self) -> (Pubkey, Pubkey) {
        (self.vault_a, self.vault_b)
    }

    fn get_quote(&self, token_in_mint: &Pubkey, amount_in: u64, current_timestamp: i64) -> Result<u64> {
        Ok(self.get_quote_with_tick_arrays(token_in_mint, amount_in, current_timestamp)?.0)
    }
}

fn find_next_initialized_tick<'a>(
    pool: &'a DecodedClmmPool,
    current_tick: i32,
    is_base_input: bool, // true = cherche vers le bas, false = cherche vers le haut
    tick_arrays: &'a BTreeMap<i32, TickArrayState>,
) -> Result<(i32, i128)> {
    if is_base_input { // Le prix baisse, on cherche un tick inférieur.
        // Itère sur tous les arrays, du plus grand au plus petit
        for (_, array_state) in tick_arrays.iter().rev() {
            // Itère sur les ticks de l'array, du plus grand au plus petit
            for i in (0..tick_array::TICK_ARRAY_SIZE).rev() {
                let tick_state = array_state.ticks[i];
                // Si le tick est initialisé ET est en dessous du tick actuel, c'est notre candidat
                if tick_state.liquidity_gross > 0 && tick_state.tick < current_tick {
                    return Ok((tick_state.tick, tick_state.liquidity_net));
                }
            }
        }
    } else { // Le prix monte, on cherche un tick supérieur.
        // Itère sur tous les arrays, du plus petit au plus grand
        for (_, array_state) in tick_arrays.iter() {
            // Itère sur les ticks de l'array, du plus petit au plus grand
            for i in 0..tick_array::TICK_ARRAY_SIZE {
                let tick_state = array_state.ticks[i];
                // Si le tick est initialisé ET est au-dessus du tick actuel, c'est notre candidat
                if tick_state.liquidity_gross > 0 && tick_state.tick > current_tick {
                    return Ok((tick_state.tick, tick_state.liquidity_net));
                }
            }
        }
    }

    Err(anyhow!("Aucun tick initialisé trouvé dans la direction du swap parmi les arrays chargés."))
}


