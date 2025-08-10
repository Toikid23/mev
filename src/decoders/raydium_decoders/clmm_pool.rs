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
use std::collections::BTreeMap;

// --- STRUCTURES (Inchangées) ---
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
    pub trade_fee_rate: u32,
    pub min_tick: i32,
    pub max_tick: i32,
    pub tick_arrays: Option<BTreeMap<i32, TickArrayState>>,
}

impl DecodedClmmPool {
    pub fn fee_as_percent(&self) -> f64 {
        self.trade_fee_rate as f64 / 10_000.0
    }
}

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

// --- LOGIQUE DE DÉCODAGE ET HYDRATATION ---
pub fn decode_pool(address: &Pubkey, data: &[u8], program_id: &Pubkey) -> Result<DecodedClmmPool> {
    const DISCRIMINATOR: [u8; 8] = [247, 237, 227, 245, 215, 195, 222, 70];
    if data.get(..8) != Some(&DISCRIMINATOR) {
        bail!("Invalid PoolState discriminator.");
    }
    let data_slice = &data[8..];
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
        trade_fee_rate: 0,
        min_tick: -443636,
        max_tick: 443636,
        tick_arrays: None,
    })
}

pub async fn hydrate(pool: &mut DecodedClmmPool, rpc_client: &RpcClient) -> Result<()> {
    let (config_res, mint_a_res, mint_b_res) = tokio::join!(
        rpc_client.get_account_data(&pool.amm_config),
        rpc_client.get_account_data(&pool.mint_a),
        rpc_client.get_account_data(&pool.mint_b)
    );
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

    const WINDOW_SIZE: i32 = 20; // Gardons une grande fenêtre pour le débogage
    let ticks_per_array = (tick_array::TICK_ARRAY_SIZE as i32) * (pool.tick_spacing as i32);
    let active_array_start_index =
        tick_array::get_start_tick_index(pool.tick_current, pool.tick_spacing);

    let mut addresses_to_fetch = Vec::new();
    for i in -WINDOW_SIZE..=WINDOW_SIZE {
        let target_start_index = active_array_start_index + (i * ticks_per_array);
        if target_start_index < clmm_math::MIN_TICK || target_start_index > clmm_math::MAX_TICK {
            continue;
        }
        let pda = tick_array::get_tick_array_address(&pool.address, target_start_index, &pool.program_id);
        addresses_to_fetch.push(pda);
    }

    let accounts_results = rpc_client.get_multiple_accounts(&addresses_to_fetch).await?;
    let mut tick_arrays = BTreeMap::new();
    for account_opt in accounts_results {
        if let Some(account) = account_opt {
            if let Ok(decoded_array) = tick_array::decode_tick_array(&account.data) {
                let start_tick = decoded_array.start_tick_index;
                tick_arrays.insert(start_tick, decoded_array);
            }
        }
    }
    pool.tick_arrays = Some(tick_arrays);
    Ok(())
}

impl PoolOperations for DecodedClmmPool {
    fn get_mints(&self) -> (Pubkey, Pubkey) {
        (self.mint_a, self.mint_b)
    }
    fn get_vaults(&self) -> (Pubkey, Pubkey) {
        (self.vault_a, self.vault_b)
    }

    fn get_quote(&self, token_in_mint: &Pubkey, amount_in: u64, _current_timestamp: i64) -> Result<u64> {
        let tick_arrays = self.tick_arrays.as_ref().ok_or_else(|| anyhow!("Pool is not hydrated."))?;
        if tick_arrays.is_empty() {
            println!("DEBUG: Aucun TickArray chargé. Quote = 0.");
            return Ok(0);
        }

        let is_base_input = if *token_in_mint == self.mint_a {
            true
        } else if *token_in_mint == self.mint_b {
            false
        } else {
            bail!("FATAL: Le token d'entrée {} n'appartient pas à ce pool.", token_in_mint);
        };

        let (in_mint_fee_bps, out_mint_fee_bps) = if is_base_input {
            (self.mint_a_transfer_fee_bps, self.mint_b_transfer_fee_bps)
        } else {
            (self.mint_b_transfer_fee_bps, self.mint_a_transfer_fee_bps)
        };

        let fee_on_input = (amount_in as u128 * in_mint_fee_bps as u128) / 10000;
        let amount_in_after_transfer_fee = amount_in.saturating_sub(fee_on_input as u64);

        let mut amount_remaining = amount_in_after_transfer_fee as u128;
        let mut total_amount_out: u128 = 0;
        let mut current_sqrt_price = self.sqrt_price_x64;
        let mut current_tick_index = self.tick_current;
        let mut current_liquidity = self.liquidity;
        let mut loop_count = 0;

        println!("\n--- DÉBUT SIMULATION DE SWAP ---");
        println!("  > État initial: amount_in={}, liquidity={}, tick_current={}, sqrt_price={}", amount_remaining, current_liquidity, current_tick_index, current_sqrt_price);
        println!("  > Direction: is_base_input = {} (Vente de {})", is_base_input, token_in_mint);
        println!("  > TickArrays chargés: {:?}", tick_arrays.keys());

        while amount_remaining > 0 {
            loop_count += 1;
            println!("\n--- Itération #{} ---", loop_count);
            println!("  > État début d'itération: amount_remaining={}, current_liquidity={}, current_tick={}", amount_remaining, current_liquidity, current_tick_index);

            if current_liquidity > 0 {
                let (next_tick_index, next_tick_liquidity_net) =
                    match find_next_initialized_tick(self, current_tick_index, is_base_input, tick_arrays) {
                        Ok(result) => result,
                        Err(e) => {
                            println!("  > FIN: Impossible de trouver le prochain tick. Erreur: {}", e);
                            break;
                        }
                    };
                println!("  > Prochain tick trouvé: index={}, net_liquidity={}", next_tick_index, next_tick_liquidity_net);

                let next_sqrt_price = clmm_math::tick_to_sqrt_price_x64(next_tick_index);

                let sqrt_price_target = if is_base_input {
                    next_sqrt_price.max(clmm_math::tick_to_sqrt_price_x64(self.min_tick))
                } else {
                    next_sqrt_price.min(clmm_math::tick_to_sqrt_price_x64(self.max_tick))
                };
                println!("  > Calcul du pas: current_sqrt_price={}, target_sqrt_price={}", current_sqrt_price, sqrt_price_target);

                let (next_sqrt_price_step, amount_in_step, amount_out_step, fee_amount_step) = clmm_math::compute_swap_step(
                    current_sqrt_price,
                    sqrt_price_target,
                    current_liquidity,
                    amount_remaining,
                    self.trade_fee_rate,
                    is_base_input,
                )?;
                println!("  > Résultat du pas: next_sqrt_price={}, amount_in={}, amount_out={}, fee={}", next_sqrt_price_step, amount_in_step, amount_out_step, fee_amount_step);

                let total_consumed = amount_in_step.saturating_add(fee_amount_step);
                if total_consumed == 0 && amount_out_step == 0 {
                    println!("  > FIN: Le pas de calcul n'a rien produit. Arrêt pour éviter une boucle infinie.");
                    break;
                }

                amount_remaining = amount_remaining.saturating_sub(total_consumed);
                total_amount_out += amount_out_step;
                current_sqrt_price = next_sqrt_price_step;

                println!("  > État fin d'itération: amount_remaining={}, total_amount_out={}", amount_remaining, total_amount_out);

                if current_sqrt_price == sqrt_price_target {
                    println!("  > Prix cible atteint. Mise à jour de la liquidité et du tick.");
                    current_liquidity = (current_liquidity as i128 + next_tick_liquidity_net) as u128;
                    current_tick_index = if is_base_input { next_tick_index - 1 } else { next_tick_index };
                }
            } else {
                println!("  > Liquidité nulle. Recherche du prochain tick pour sauter.");
                let (next_tick_index, next_tick_liquidity_net) = match find_next_initialized_tick(self, current_tick_index, is_base_input, tick_arrays) {
                    Ok(result) => result,
                    Err(e) => {
                        println!("  > FIN: Impossible de trouver le prochain tick après une liquidité nulle. Erreur: {}", e);
                        break;
                    }
                };
                println!("  > Saut vers tick: index={}, nouvelle liquidité={}", next_tick_index, next_tick_liquidity_net);
                current_tick_index = if is_base_input { next_tick_index - 1 } else { next_tick_index };
                current_sqrt_price = clmm_math::tick_to_sqrt_price_x64(next_tick_index);
                current_liquidity = (current_liquidity as i128 + next_tick_liquidity_net) as u128;
            }
            if loop_count > 40 {
                println!("  > SÉCURITÉ: Plus de 40 itérations, arrêt de la simulation.");
                break;
            }
        }

        println!("\n--- FIN SIMULATION DE SWAP ---");
        println!("  > Montant de sortie total (brut): {}", total_amount_out);

        let fee_on_output = (total_amount_out * out_mint_fee_bps as u128) / 10000;
        let final_amount_out = total_amount_out.saturating_sub(fee_on_output);
        println!("  > Frais de transfert en sortie: {}", fee_on_output);
        println!("  > Montant de sortie final (net): {}", final_amount_out);

        Ok(final_amount_out as u64)
    }
}

// --- `find_next_initialized_tick` CORRIGÉ FINAL ---
fn find_next_initialized_tick<'a>(
    pool: &'a DecodedClmmPool,
    current_tick: i32,
    is_base_input: bool, // true = cherche vers le bas, false = cherche vers le haut
    tick_arrays: &'a BTreeMap<i32, TickArrayState>,
) -> Result<(i32, i128)> {
    if is_base_input { // Le prix baisse, on cherche un tick inférieur.
        // On itère sur tous les arrays chargés, en ordre DÉCROISSANT de start_tick_index.
        for (_, array_state) in tick_arrays.iter().rev() {
            // On parcourt les ticks de l'array du plus grand au plus petit index (i).
            for i in (0..tick_array::TICK_ARRAY_SIZE).rev() {
                let tick_state = &array_state.ticks[i];
                // On retourne le premier tick initialisé qu'on trouve qui est strictement inférieur au tick actuel.
                if tick_state.liquidity_gross > 0 && tick_state.tick < current_tick {
                    return Ok((tick_state.tick, tick_state.liquidity_net));
                }
            }
        }
    } else { // Le prix monte, on cherche un tick supérieur.
        // On itère sur tous les arrays chargés, en ordre CROISSANT de start_tick_index.
        for (_, array_state) in tick_arrays.iter() {
            // On parcourt les ticks de l'array du plus petit au plus grand index (i).
            for i in 0..tick_array::TICK_ARRAY_SIZE {
                let tick_state = &array_state.ticks[i];
                // On retourne le premier tick initialisé qu'on trouve qui est strictement supérieur au tick actuel.
                if tick_state.liquidity_gross > 0 && tick_state.tick > current_tick {
                    return Ok((tick_state.tick, tick_state.liquidity_net));
                }
            }
        }
    }
    Err(anyhow!("Aucun tick initialisé trouvé dans la direction du swap parmi les arrays chargés."))
}