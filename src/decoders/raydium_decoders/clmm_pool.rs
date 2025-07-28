use crate::decoders::pool_operations::PoolOperations;
use crate::decoders::raydium_decoders::tick_array::{self, TickArrayState};
use crate::math::clmm_math;
use crate::decoders::raydium_decoders::clmm_config;
use solana_sdk::pubkey::Pubkey;
use anyhow::{bail, Result, anyhow};
use std::collections::BTreeMap;
use solana_client::{
    rpc_config::{RpcAccountInfoConfig, RpcProgramAccountsConfig},
    rpc_filter::{Memcmp, RpcFilterType},
};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_account_decoder::UiAccountEncoding;
use bytemuck::{from_bytes, Pod, Zeroable};
use crate::decoders::spl_token_decoders;

// Structure de sortie
#[derive(Debug, Clone)]
pub struct DecodedClmmPool {
    pub address: Pubkey,
    pub program_id: Pubkey,
    pub amm_config: Pubkey,
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
    pub total_fee_percent: f64,
    pub min_tick: i32,
    pub max_tick: i32,
    pub tick_arrays: Option<BTreeMap<i32, TickArrayState>>,
}

impl DecodedClmmPool {
    pub fn fee_as_percent(&self) -> f64 {
        self.total_fee_percent * 100.0
    }
}

// Structure de décodage (simple et qui fonctionnait)
#[repr(C, packed)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
struct PoolStateData {
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
}

// Fonction de décodage
pub fn decode_pool(address: &Pubkey, data: &[u8], program_id: &Pubkey) -> Result<DecodedClmmPool> {
    const DISCRIMINATOR: [u8; 8] = [247, 237, 227, 245, 215, 195, 222, 70];
    if data.get(..8) != Some(&DISCRIMINATOR) {
        bail!("Invalid PoolState discriminator.");
    }
    let data_slice = &data[8..];

    // On utilise la méthode qui prend en compte la taille de notre structure
    let pool_struct: &PoolStateData = from_bytes(&data_slice[..std::mem::size_of::<PoolStateData>()]);

    Ok(DecodedClmmPool {
        address: *address,
        program_id: *program_id,
        amm_config: pool_struct.amm_config,
        mint_a: pool_struct.token_mint_0,
        mint_b: pool_struct.token_mint_1,
        vault_a: pool_struct.token_vault_0,
        vault_b: pool_struct.token_vault_1,
        tick_spacing: pool_struct.tick_spacing,
        liquidity: pool_struct.liquidity,
        sqrt_price_x64: pool_struct.sqrt_price_x64,
        tick_current: pool_struct.tick_current,
        mint_a_decimals: 0,
        mint_b_decimals: 0,
        mint_a_transfer_fee_bps: 0,
        mint_b_transfer_fee_bps: 0,
        total_fee_percent: 0.0,
        min_tick: -443636,
        max_tick: 443636,
        tick_arrays: None,
    })
}

// Fonction d'hydratation (version lente mais fonctionnelle)
pub async fn hydrate(pool: &mut DecodedClmmPool, rpc_client: &RpcClient) -> Result<()> {
    let (config_res, mint_a_res, mint_b_res) = tokio::join!(
        rpc_client.get_account_data(&pool.amm_config),
        rpc_client.get_account_data(&pool.mint_a),
        rpc_client.get_account_data(&pool.mint_b)
    );

    let config_account_data = config_res?;
    let decoded_config = clmm_config::decode_config(&config_account_data)?;
    pool.tick_spacing = decoded_config.tick_spacing;
    pool.total_fee_percent = decoded_config.trade_fee_rate as f64 / 1_000_000.0;

    let mint_a_data = mint_a_res?;
    let decoded_mint_a = spl_token_decoders::mint::decode_mint(&pool.mint_a, &mint_a_data)?;
    pool.mint_a_decimals = decoded_mint_a.decimals;
    pool.mint_a_transfer_fee_bps = decoded_mint_a.transfer_fee_basis_points;

    let mint_b_data = mint_b_res?;
    let decoded_mint_b = spl_token_decoders::mint::decode_mint(&pool.mint_b, &mint_b_data)?;
    pool.mint_b_decimals = decoded_mint_b.decimals;
    pool.mint_b_transfer_fee_bps = decoded_mint_b.transfer_fee_basis_points;

    let tick_array_accounts = find_tick_arrays_for_pool(rpc_client, &pool.program_id, &pool.address).await?;

    let mut tick_arrays = BTreeMap::new();
    for (_pubkey, account) in tick_array_accounts.iter() {
        if let Ok(decoded_array) = tick_array::decode_tick_array(&account.data) {
            tick_arrays.insert(decoded_array.start_tick_index, decoded_array);
        }
    }
    pool.tick_arrays = Some(tick_arrays);

    Ok(())
}

async fn find_tick_arrays_for_pool(
    client: &RpcClient,
    program_id: &Pubkey,
    pool_id: &Pubkey,
) -> Result<Vec<(Pubkey, solana_sdk::account::Account)>> {
    const POOL_ID_OFFSET: usize = 8;
    const TICK_ARRAY_ACCOUNT_SIZE: u64 = (8 + std::mem::size_of::<TickArrayState>()) as u64;

    let filters = vec![
        RpcFilterType::DataSize(TICK_ARRAY_ACCOUNT_SIZE),
        RpcFilterType::Memcmp(Memcmp::new_base58_encoded(
            POOL_ID_OFFSET,
            &pool_id.to_bytes(),
        )),
    ];

    let accounts = client.get_program_accounts_with_config(
        program_id,
        RpcProgramAccountsConfig {
            filters: Some(filters),
            account_config: RpcAccountInfoConfig {
                encoding: Some(UiAccountEncoding::Base64),
                ..RpcAccountInfoConfig::default() // <--- LIGNE CORRECTE
            },
            with_context: Some(false), // C'est une bonne pratique de l'ajouter
            ..RpcProgramAccountsConfig::default()
        },
    ).await?;
    Ok(accounts)
}

impl PoolOperations for DecodedClmmPool {
    fn get_mints(&self) -> (Pubkey, Pubkey) { (self.mint_a, self.mint_b) }
    fn get_vaults(&self) -> (Pubkey, Pubkey) { (self.vault_a, self.vault_b) }
    fn get_quote(&self, token_in_mint: &Pubkey, amount_in: u64) -> Result<u64> {
        // --- 1. Appliquer les frais de transfert sur l'INPUT ---
        let (in_mint_fee_bps, out_mint_fee_bps) = if *token_in_mint == self.mint_a {
            (self.mint_a_transfer_fee_bps, self.mint_b_transfer_fee_bps)
        } else {
            (self.mint_b_transfer_fee_bps, self.mint_a_transfer_fee_bps)
        };

        let fee_on_input = (amount_in as u128 * in_mint_fee_bps as u128) / 10000;
        let amount_in_after_transfer_fee = amount_in.saturating_sub(fee_on_input as u64);

        // --- 2. Calculer le swap avec le montant NET ---
        let tick_arrays = self.tick_arrays.as_ref().ok_or_else(|| anyhow!("TickArrays not hydrated"))?;
        if tick_arrays.is_empty() { return Err(anyhow!("No initialized TickArrays found")); }

        let gross_amount_out = get_clmm_quote(
            self,
            amount_in_after_transfer_fee, // On utilise le montant net
            token_in_mint,
            tick_arrays
        )?;

        // --- 3. Appliquer les frais de transfert sur l'OUTPUT ---
        let fee_on_output = (gross_amount_out as u128 * out_mint_fee_bps as u128) / 10000;
        let final_amount_out = gross_amount_out.saturating_sub(fee_on_output as u64);

        Ok(final_amount_out)
    }
}

pub fn get_clmm_quote(
    pool: &DecodedClmmPool, amount_in: u64, token_in_mint: &Pubkey, tick_arrays: &BTreeMap<i32, TickArrayState>
) -> Result<u64> {
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
        } else {
            // Si on n'a pas atteint le tick, c'est que tout le montant a été consommé.
            // On met à jour le tick_current pour refléter le nouveau prix.
            // (Cette partie est complexe et peut être omise pour un simple quote)
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
    let current_array_start_index = tick_array::get_start_tick_index(current_tick, pool.tick_spacing);

    if is_base_input { // Cherche vers le bas
        // Itère sur les arrays en ordre décroissant à partir de l'array courant
        for (_, array_state) in tick_arrays.range(..=current_array_start_index).rev() {
            // Itère sur les ticks de l'array, de la fin vers le début
            for i in (0..tick_array::TICK_ARRAY_SIZE).rev() {
                let tick_state = &array_state.ticks[i];
                if tick_state.liquidity_gross > 0 && tick_state.tick < current_tick {
                    return Ok((tick_state.tick, tick_state.liquidity_net));
                }
            }
        }
    } else { // Cherche vers le haut
        // Itère sur les arrays en ordre croissant à partir de l'array courant
        for (_, array_state) in tick_arrays.range(current_array_start_index..) {
            // Itère sur les ticks de l'array, du début vers la fin
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