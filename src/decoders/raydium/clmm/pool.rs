
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
                    Err(_) => (if is_base_input { math::MIN_TICK } else { math::MAX_TICK }, 0)
                };

                let sqrt_price_target = math::tick_to_sqrt_price_x64(next_tick_index);

                let (next_sqrt_price, amount_in_step, amount_out_step, fee_amount_step) = math::compute_swap_step(
                    current_sqrt_price, sqrt_price_target, current_liquidity, amount_remaining, self.trade_fee_rate, is_base_input,
                )?;

                let total_consumed = amount_in_step.saturating_add(fee_amount_step);
                if total_consumed == 0 { break; }

                amount_remaining = amount_remaining.saturating_sub(total_consumed);
                total_amount_out = total_amount_out.saturating_add(amount_out_step);
                current_sqrt_price = next_sqrt_price;
            }

            if current_sqrt_price == math::tick_to_sqrt_price_x64(find_next_initialized_tick(self, current_tick_index, is_base_input, tick_arrays)?.0) {
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
        last_swap_timestamp: 0,
    })
}

// REMPLACEZ VOTRE FONCTION HYDRATE PAR CELLE-CI (VERSION CORRIGÉE POUR COMPILER) :
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

    let mint_b_data = mint_b_res?;
    let decoded_mint_b = spl_token_decoders::mint::decode_mint(&pool.mint_b, &mint_b_data)?;
    pool.mint_b_decimals = decoded_mint_b.decimals;
    pool.mint_b_transfer_fee_bps = decoded_mint_b.transfer_fee_basis_points;

    // --- Étape 2: Lecture ROBUSTE des bitmaps ---
    let pool_state_data = pool_state_res?;

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

    // Le reste de la logique reste le même, car il était déjà correct.
    let extension_bitmap_words = if let Ok(account) = bitmap_ext_res {

        tickarray_bitmap_extension::decode_tick_array_bitmap_extension(&account.data)?.bitmap_words
    } else {

        Vec::new()
    };

    // ... (le reste de la fonction est inchangé)

    let mut addresses_to_fetch = Vec::new();
    let multiplier = (tick_array::TICK_ARRAY_SIZE as i32) * (pool.tick_spacing as i32);

    for (word_index, &word) in default_bitmap.iter().enumerate() {
        if word == 0 { continue; }
        for bit_index in 0..64 {
            if (word & (1 << bit_index)) != 0 {
                let compressed_index = word_index * 64 + bit_index;
                let start_tick_index = (compressed_index as i32 - 512) * multiplier;
                if start_tick_index >= math::MIN_TICK && start_tick_index <= math::MAX_TICK {
                    let address = tick_array::get_tick_array_address(&pool.address, start_tick_index, &pool.program_id);
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
                if start_tick_index >= math::MIN_TICK && start_tick_index <= math::MAX_TICK {
                    let address = tick_array::get_tick_array_address(&pool.address, start_tick_index, &pool.program_id);
                    addresses_to_fetch.push(address);
                }
            }
        }
    }


    if addresses_to_fetch.is_empty() {
        pool.tick_arrays = Some(BTreeMap::new());

        return Ok(());
    }

    let accounts_results = rpc_client.get_multiple_accounts(&addresses_to_fetch).await?;
    let found_accounts_count = accounts_results.iter().filter(|a| a.is_some()).count();


    let mut tick_arrays = BTreeMap::new();
    for account_opt in accounts_results {
        if let Some(account) = account_opt {
            match tick_array::decode_tick_array(&account.data) {
                Ok(decoded_array) => {
                    let start_tick_index = decoded_array.start_tick_index;

                    tick_arrays.insert(start_tick_index, decoded_array);
                }
                Err(e) => {

                }
            }
        }
    }

    let tick_arrays_count = tick_arrays.len();
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

    fn address(&self) -> Pubkey { self.address }

    fn get_quote(&self, token_in_mint: &Pubkey, amount_in: u64, current_timestamp: i64) -> Result<u64> {
        // --- LA LOGIQUE FINALE ET COMPLÈTE ---

        // 1. Calculer le montant brut de sortie en utilisant notre logique de swap validée.
        // Cette fonction prend déjà en compte la taxe sur le jeton d'entrée.
        let (gross_amount_out, _) =
            self.get_quote_with_tick_arrays(token_in_mint, amount_in, current_timestamp)?;

        // 2. Déterminer la taxe du jeton de SORTIE.
        let output_mint_fee_bps = if *token_in_mint == self.mint_a {
            self.mint_b_transfer_fee_bps // La sortie est le token B
        } else {
            self.mint_a_transfer_fee_bps // La sortie est le token A
        };

        // 3. Calculer la taxe sur le montant brut de sortie.
        let fee_on_output = (gross_amount_out as u128 * output_mint_fee_bps as u128) / 10000;

        // 4. Calculer le montant final que l'utilisateur reçoit.
        let final_amount_out = gross_amount_out.saturating_sub(fee_on_output as u64);

        Ok(final_amount_out)
    }
    async fn get_quote_async(&mut self, token_in_mint: &Pubkey, amount_in: u64, _rpc_client: &RpcClient) -> Result<u64> {
        self.get_quote(token_in_mint, amount_in, 0)
    }

    fn create_swap_instruction(
        &self,
        token_in_mint: &Pubkey,
        amount_in: u64,
        min_amount_out: u64, // Renommé pour correspondre au trait
        user_accounts: &UserSwapAccounts,
    ) -> Result<Instruction> {

        // Étape 1 : Calculer les tick_arrays requis en interne (logique prise de votre test)
        let is_base_input = *token_in_mint == self.mint_a;
        let mut tick_arrays = self.get_next_initialized_tick_arrays(is_base_input, 3);
        if tick_arrays.is_empty() {
            bail!("Impossible de trouver des tick_arrays initialisés pour le swap CLMM.");
        }
        while tick_arrays.len() < 3 {
            tick_arrays.push(*tick_arrays.last().unwrap());
        }

        // --- DÉBUT DE VOTRE CODE ORIGINAL, LÉGÈREMENT ADAPTÉ ---
        let sqrt_price_limit = if is_base_input {
            4295128739_u128 // MIN_SQRT_PRICE_X64 + 1
        } else {
            79226673515401279992447579055_u128 // MAX_SQRT_PRICE_X64 - 1
        };

        let mut instruction_data: Vec<u8> = vec![43, 4, 237, 11, 26, 201, 30, 98]; // Discriminator `swap_v2`
        instruction_data.extend_from_slice(&amount_in.to_le_bytes());
        instruction_data.extend_from_slice(&min_amount_out.to_le_bytes()); // Utilise le nouvel argument
        instruction_data.extend_from_slice(&sqrt_price_limit.to_le_bytes());
        instruction_data.push(u8::from(is_base_input));

        let (input_vault, output_vault, input_vault_mint, output_vault_mint) = if is_base_input {
            (self.vault_a, self.vault_b, self.mint_a, self.mint_b)
        } else {
            (self.vault_b, self.vault_a, self.mint_b, self.mint_a)
        };

        let mut accounts = vec![
            // On utilise les champs de `user_accounts` pour remplacer les anciens arguments
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

        // On utilise le vecteur `tick_arrays` qu'on a calculé au début de la fonction
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

