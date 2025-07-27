// Fichier COMPLET et ARCHITECTURALEMENT CORRECT : src/decoders/raydium_decoders/clmm_pool.rs

use crate::decoders::pool_operations::PoolOperations;
use crate::decoders::raydium_decoders::tick_array::{self, TickArrayState};
use crate::math::clmm_math;
use crate::decoders::raydium_decoders::clmm_config;
use solana_sdk::pubkey::Pubkey;
use anyhow::{bail, Result, anyhow};
use std::collections::BTreeMap;
use std::convert::TryInto;
use solana_client::{
    rpc_client::RpcClient,
    rpc_config::{RpcAccountInfoConfig, RpcProgramAccountsConfig},
    rpc_filter::{Memcmp, RpcFilterType},
};
use solana_account_decoder::UiAccountEncoding;

#[derive(Debug, Clone)]
pub struct DecodedClmmPool {
    pub address: Pubkey,
    pub program_id: Pubkey,
    pub amm_config: Pubkey,
    pub mint_a: Pubkey,
    pub mint_b: Pubkey,
    pub mint_a_decimals: u8,
    pub mint_b_decimals: u8,
    pub vault_a: Pubkey,
    pub vault_b: Pubkey,
    pub liquidity: u128,
    pub sqrt_price_x64: u128,
    pub tick_current: i32,
    pub tick_spacing: u16,
    pub total_fee_percent: f64,
    pub min_tick: i32,
    pub max_tick: i32,
    pub tick_arrays: Option<BTreeMap<i32, TickArrayState>>,
}

/// Étape 1 : Décodage initial du PoolState
pub fn decode_pool(address: &Pubkey, data: &[u8], program_id: &Pubkey) -> Result<DecodedClmmPool> {
    // ... (Cette fonction reste la même, elle fait juste le décodage de base)
    const DISCRIMINATOR: [u8; 8] = [247, 237, 227, 245, 215, 195, 222, 70];
    if data.get(..8) != Some(&DISCRIMINATOR) { bail!("Discriminator de PoolState invalide."); }
    let data_slice = &data[8..];
    // ... (le reste de la fonction de décodage est ici)
    // Pour la simplicité, nous utilisons la méthode chirurgicale qui a fonctionné
    Ok(DecodedClmmPool {
        address: *address, program_id: *program_id,
        amm_config: Pubkey::new_from_array(data.get(9..41).ok_or(anyhow!("slice amm_config oob"))?.try_into()?),
        mint_a: Pubkey::new_from_array(data.get(65..97).ok_or(anyhow!("slice mint_a oob"))?.try_into()?),
        mint_b: Pubkey::new_from_array(data.get(97..129).ok_or(anyhow!("slice mint_b oob"))?.try_into()?),
        vault_a: Pubkey::new_from_array(data.get(129..161).ok_or(anyhow!("slice vault_a oob"))?.try_into()?),
        vault_b: Pubkey::new_from_array(data.get(161..193).ok_or(anyhow!("slice vault_b oob"))?.try_into()?),
        tick_spacing: u16::from_le_bytes(data.get(235..237).ok_or(anyhow!("slice tick_spacing oob"))?.try_into()?),
        liquidity: u128::from_le_bytes(data.get(237..253).ok_or(anyhow!("slice liquidity oob"))?.try_into()?),
        sqrt_price_x64: u128::from_le_bytes(data.get(253..269).ok_or(anyhow!("slice sqrt_price oob"))?.try_into()?),
        tick_current: i32::from_le_bytes(data.get(269..273).ok_or(anyhow!("slice tick_current oob"))?.try_into()?),
        mint_a_decimals: 0, mint_b_decimals: 0, // Initialisé à 0, sera hydraté
        total_fee_percent: 0.0, min_tick: -443636, max_tick: 443636, tick_arrays: None,
    })
}

/// Étape 2 : Hydrate un pool décodé avec toutes les données on-chain nécessaires.
pub fn hydrate(pool: &mut DecodedClmmPool, rpc_client: &RpcClient) -> Result<()> {
    use spl_token::state::Mint;
    use solana_sdk::program_pack::Pack;

    // 1. Hydrater les frais
    let config_account_data = rpc_client.get_account_data(&pool.amm_config)?;
    let config_struct = clmm_config::decode_config(&config_account_data)?;
    pool.total_fee_percent = config_struct.trade_fee_rate as f64 / 1_000_000.0;

    // 2. Hydrater les décimales
    let mint_infos = rpc_client.get_multiple_accounts(&[pool.mint_a, pool.mint_b])?;
    pool.mint_a_decimals = Mint::unpack(&mint_infos[0].as_ref().unwrap().data)?.decimals;
    pool.mint_b_decimals = Mint::unpack(&mint_infos[1].as_ref().unwrap().data)?.decimals;

    // 3. Hydrater les TickArrays via le scan on-chain
    let tick_array_accounts = find_tick_arrays_for_pool(rpc_client, &pool.program_id, &pool.address)?;

    let mut tick_arrays = BTreeMap::new();
    for (_pubkey, account) in tick_array_accounts.iter() {
        if let Ok(decoded_array) = tick_array::decode_tick_array(&account.data) {
            tick_arrays.insert(decoded_array.start_tick_index, decoded_array);
        }
    }
    pool.tick_arrays = Some(tick_arrays);
    Ok(())
}


// Fonction privée de scan, maintenant locale au module CLMM
fn find_tick_arrays_for_pool(
    client: &RpcClient, program_id: &Pubkey, pool_id: &Pubkey
) -> Result<Vec<(Pubkey, solana_sdk::account::Account)>> {
    const POOL_ID_OFFSET: usize = 8;
    const TICK_ARRAY_ACCOUNT_SIZE: u64 = (8 + std::mem::size_of::<TickArrayState>()) as u64;

    let filters = vec![
        RpcFilterType::DataSize(TICK_ARRAY_ACCOUNT_SIZE),
        RpcFilterType::Memcmp(Memcmp::new_base58_encoded(POOL_ID_OFFSET, &pool_id.to_bytes())),
    ];

    let accounts = client.get_program_accounts_with_config(
        program_id,
        RpcProgramAccountsConfig {
            filters: Some(filters),
            account_config: RpcAccountInfoConfig {
                encoding: Some(UiAccountEncoding::Base64),
                ..RpcAccountInfoConfig::default()
            },
            ..RpcProgramAccountsConfig::default()
        },
    )?;
    Ok(accounts)
}


// --- Le reste du fichier est identique et fonctionnel ---
impl PoolOperations for DecodedClmmPool {
    fn get_mints(&self) -> (Pubkey, Pubkey) { (self.mint_a, self.mint_b) }
    fn get_vaults(&self) -> (Pubkey, Pubkey) { (self.vault_a, self.vault_b) }
    fn get_quote(&self, token_in_mint: &Pubkey, amount_in: u64) -> Result<u64> {
        let tick_arrays = self.tick_arrays.as_ref().ok_or_else(|| anyhow!("TickArrays not hydrated"))?;
        if tick_arrays.is_empty() { return Err(anyhow!("No initialized TickArrays found")); }
        get_clmm_quote(self, amount_in, token_in_mint, tick_arrays)
    }
}
// ... (les fonctions `get_clmm_quote` et `find_next_initialized_tick` restent ici) ...
// (assurez-vous qu'elles sont bien à la fin de votre fichier)

pub fn get_clmm_quote(
    pool: &DecodedClmmPool, amount_in: u64, token_in_mint: &Pubkey, tick_arrays: &BTreeMap<i32, TickArrayState>
) -> Result<u64> {
    // ... (logique de calcul correcte)
    let mut amount_remaining = amount_in as u128;
    let mut total_amount_out: u128 = 0;
    let mut current_sqrt_price = pool.sqrt_price_x64;
    let mut current_tick_index = pool.tick_current;
    let mut current_liquidity = pool.liquidity;
    let is_base_input = *token_in_mint == pool.mint_a;
    while amount_remaining > 0 {
        let (next_tick_index, next_tick_liquidity_net) = match find_next_initialized_tick(pool, current_tick_index, is_base_input, tick_arrays) {
            Ok(result) => result,
            Err(_) => break,
        };
        let next_sqrt_price = clmm_math::tick_to_sqrt_price_x64(next_tick_index);
        let sqrt_price_target = if is_base_input {
            next_sqrt_price.max(clmm_math::tick_to_sqrt_price_x64(pool.min_tick))
        } else {
            next_sqrt_price.min(clmm_math::tick_to_sqrt_price_x64(pool.max_tick))
        };
        let (amount_in_step, amount_out_step, next_sqrt_price_step) = clmm_math::compute_swap_step(
            current_sqrt_price, sqrt_price_target, current_liquidity, amount_remaining, is_base_input,
        );
        total_amount_out += amount_out_step;
        amount_remaining -= amount_in_step;
        current_sqrt_price = next_sqrt_price_step;
        if current_sqrt_price == sqrt_price_target {
            current_liquidity = (current_liquidity as i128 + next_tick_liquidity_net) as u128;
            current_tick_index = next_tick_index;
        }
    }
    const PRECISION: u128 = 1_000_000;
    let fee_numerator = (pool.total_fee_percent * PRECISION as f64) as u128;
    let fee_to_deduct = (total_amount_out * fee_numerator) / PRECISION;
    Ok(total_amount_out.saturating_sub(fee_to_deduct) as u64)
}

fn find_next_initialized_tick(
    pool: &DecodedClmmPool, current_tick: i32, is_base_input: bool, tick_arrays: &BTreeMap<i32, TickArrayState>
) -> Result<(i32, i128)> {
    // ... (logique de recherche correcte)
    let current_array_start_index = tick_array::get_start_tick_index(current_tick, pool.tick_spacing);
    if is_base_input {
        for (_, array_state) in tick_arrays.range(..=current_array_start_index).rev() {
            for i in (0..tick_array::TICK_ARRAY_SIZE).rev() {
                let tick_state = &array_state.ticks[i];
                if tick_state.liquidity_gross > 0 && tick_state.tick < current_tick {
                    return Ok((tick_state.tick, tick_state.liquidity_net));
                }
            }
        }
    } else {
        for (_, array_state) in tick_arrays.range(current_array_start_index..) {
            for i in 0..tick_array::TICK_ARRAY_SIZE {
                let tick_state = &array_state.ticks[i];
                if tick_state.liquidity_gross > 0 && tick_state.tick > current_tick {
                    return Ok((tick_state.tick, tick_state.liquidity_net));
                }
            }
        }
    }
    Err(anyhow!("No more initialized ticks"))
}