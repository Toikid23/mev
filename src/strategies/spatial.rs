// DANS : src/strategies/spatial.rs

use crate::decoders::{Pool, PoolOperations};
use crate::graph_engine::Graph;
use solana_sdk::pubkey::Pubkey;
use std::collections::HashMap;
use std::sync::Arc;
use anyhow::{Result};
use crate::execution::cu_manager;
use crate::config::Config;
use tracing::{trace, info, warn, debug};

#[derive(Debug, Clone)]
pub struct ArbitrageOpportunity {
    pub profit_in_lamports: u64,
    pub amount_in: u64,
    pub token_in_mint: Pubkey,
    pub token_intermediate_mint: Pubkey,
    pub pool_buy_from_key: Pubkey,
    pub pool_sell_to_key: Pubkey,
}

pub async fn find_spatial_arbitrage(
    graph: Arc<Graph>,
    config: &Config,
) -> Vec<ArbitrageOpportunity> {
    const MINIMUM_PROFIT_LAMPS: u64 = 100_000;
    const PRICE_CHECK_AMOUNT: u64 = 100_000_000;

    // --- BLOC DE CALCUL DU BUDGET (CORRIGÉ) ---
    // Le capital de base pour nos trades est le wSOL, qui a 9 décimales.
    const WSOL_DECIMALS: i32 = 9;
    let max_trade_size_lamports = (config.max_trade_size_sol * 10f64.powi(WSOL_DECIMALS)) as u64;
    // --- FIN DU BLOC ---

    let mut opportunities = Vec::with_capacity(10);

    let pools_by_pair = {
        let mut map: HashMap<(Pubkey, Pubkey), Vec<Pubkey>> = HashMap::with_capacity(graph.pools.len() / 2);
        for (pool_key, pool_data) in graph.pools.iter() {
            let (mut mint_a, mut mint_b) = pool_data.get_mints();
            if mint_a > mint_b { std::mem::swap(&mut mint_a, &mut mint_b); }
            map.entry((mint_a, mint_b)).or_default().push(*pool_key);
        }
        map
    };

    for ((mint_a, mint_b), pool_keys) in pools_by_pair.iter() {
        if pool_keys.len() < 2 { continue; }

        let mut best_seller: Option<(u64, Pubkey)> = None;
        let mut best_buyer: Option<(u64, Pubkey)> = None;

        debug!(
            mint_a = %mint_a,
            mint_b = %mint_b,
            pool_count = pool_keys.len(),
            "Analyse d'une paire de tokens pour arbitrage spatial"
        );

        for pool_key in pool_keys {
            let pool_data = match graph.pools.get(pool_key) {
                Some(p) => p,
                None => continue,
            };

            if let Ok(quote_result_a_to_b) = pool_data.get_quote_with_details(mint_a, PRICE_CHECK_AMOUNT, 0) {
                if quote_result_a_to_b.amount_out > 0 {
                    let price_a_per_b = (PRICE_CHECK_AMOUNT as u128 * 1_000_000) / quote_result_a_to_b.amount_out as u128;
                    if best_seller.is_none() || price_a_per_b < best_seller.unwrap().0 as u128 {
                        best_seller = Some((price_a_per_b as u64, *pool_key));
                    }
                }
            }

            if let Ok(quote_result_b_to_a) = pool_data.get_quote_with_details(mint_b, PRICE_CHECK_AMOUNT, 0) {
                if quote_result_b_to_a.amount_out > 0 {
                    let price_a_per_b = (quote_result_b_to_a.amount_out as u128 * 1_000_000) / PRICE_CHECK_AMOUNT as u128;
                    if best_buyer.is_none() || price_a_per_b > best_buyer.unwrap().0 as u128 {
                        best_buyer = Some((price_a_per_b as u64, *pool_key));
                    }
                }
            }
        }

        if let (Some((sell_price_norm, sell_key)), Some((buy_price_norm, buy_key))) = (best_seller, best_buyer) {
            if sell_key != buy_key && buy_price_norm > sell_price_norm {

                info!(
                    mint_a = %mint_a,
                    mint_b = %mint_b,
                    buy_from_pool = %sell_key,
                    sell_to_pool = %buy_key,
                    buy_price = sell_price_norm,
                    sell_price = buy_price_norm,
                    profit_margin_percent = (buy_price_norm as f64 - sell_price_norm as f64) / sell_price_norm as f64 * 100.0,
                    "Écart de prix prometteur trouvé"
                );

                let initial_profit_estimate_percent = (buy_price_norm as f64 - sell_price_norm as f64) / sell_price_norm as f64;

                let mut pool_buy_from = match graph.pools.get(&sell_key) {
                    Some(p) => p.clone(),
                    None => continue,
                };
                let mut pool_sell_to = match graph.pools.get(&buy_key) {
                    Some(p) => p.clone(),
                    None => continue,
                };

                let estimated_cus = cu_manager::estimate_arbitrage_cost(&pool_buy_from, 1, &pool_sell_to, 1);
                const ESTIMATED_PRIORITY_FEE_PER_CU: u64 = 5000;
                let estimated_rpc_cost = (estimated_cus * ESTIMATED_PRIORITY_FEE_PER_CU) / 1_000_000;
                const JITO_TIP_PERCENT: u64 = 20;
                let estimated_profit_on_test = (PRICE_CHECK_AMOUNT as f64 * initial_profit_estimate_percent) as u64;
                let estimated_jito_tip = (estimated_profit_on_test as u128 * JITO_TIP_PERCENT as u128 / 100) as u64;
                let transaction_cost_threshold = std::cmp::max(estimated_rpc_cost, estimated_jito_tip);

                // --- LA CONDITION IF EST MAINTENANT CORRECTE ---
                if estimated_profit_on_test > transaction_cost_threshold {
                    if let Some(final_opportunity) = find_optimal_arbitrage(
                        &mut pool_buy_from,
                        &mut pool_sell_to,
                        *mint_a,
                        *mint_b,
                        max_trade_size_lamports,
                    ) {
                        if final_opportunity.profit_in_lamports >= MINIMUM_PROFIT_LAMPS {
                            opportunities.push(final_opportunity);
                        }
                    }
                }
                // --- FIN DE LA CORRECTION ---
            }
        }
    }
    if !opportunities.is_empty() {
        trace!(length = opportunities.len(), "spatial.rs - opportunities length");
    }
    if !pools_by_pair.is_empty() {
        trace!(length = pools_by_pair.len(), "spatial.rs - pools_by_pair length");
    }

    opportunities
}

fn find_optimal_arbitrage(
    pool_buy_from: &mut Pool,
    pool_sell_to: &mut Pool,
    token_in_mint: Pubkey,
    token_intermediate_mint: Pubkey,
    // --- NOUVEAU PARAMÈTRE ---
    max_trade_size_lamports: u64,
) -> Option<ArbitrageOpportunity> {
    let ts = 0;

    let (buy_res_a, buy_res_b) = pool_buy_from.get_reserves();
    let (sell_res_a, sell_res_b) = pool_sell_to.get_reserves();
    let (buy_from_intermediate_reserve, _) = if pool_buy_from.get_mints().0 == token_intermediate_mint { (buy_res_a, buy_res_b) } else { (buy_res_b, buy_res_a) };
    let (sell_to_intermediate_reserve, _) = if pool_sell_to.get_mints().0 == token_intermediate_mint { (sell_res_a, sell_res_b) } else { (sell_res_b, sell_res_a) };

    let low_bound: u64 = 0;
    let mut high_bound_from_liquidity: u64 = std::cmp::min(buy_from_intermediate_reserve, sell_to_intermediate_reserve);
    if high_bound_from_liquidity == 0 { high_bound_from_liquidity = 500 * 1_000_000_000; }

    // --- LA MODIFICATION CLÉ EST ICI ---
    // On s'assure que notre recherche ne dépasse JAMAIS notre budget.
    let high_bound = std::cmp::min(high_bound_from_liquidity, max_trade_size_lamports);

    let profit_fn = |intermediate_amount: u64| -> Result<i128> {
        if intermediate_amount == 0 {
            return Ok(0);
        }
        let cost = pool_buy_from.get_required_input(&token_intermediate_mint, intermediate_amount, ts)?;
        let revenue = pool_sell_to.get_quote(&token_intermediate_mint, intermediate_amount, ts)?;

        Ok(revenue as i128 - cost as i128)
    };

    if let Some((best_amount_intermediate, max_profit)) = ternary_search_optimizer(low_bound, high_bound, profit_fn) {
        if let Ok(final_cost) = pool_buy_from.get_required_input(&token_intermediate_mint, best_amount_intermediate, ts) {
            let opportunity = ArbitrageOpportunity {
                profit_in_lamports: max_profit as u64,
                amount_in: final_cost,
                token_in_mint,
                token_intermediate_mint,
                pool_buy_from_key: pool_buy_from.address(),
                pool_sell_to_key: pool_sell_to.address(),
            };
            return Some(opportunity);
        }
    }

    None
}

fn ternary_search_optimizer<F>(
    mut low_bound: u64,
    mut high_bound: u64,
    mut profit_fn: F,
) -> Option<(u64, i128)>
where
    F: FnMut(u64) -> Result<i128>,
{
    let mut max_profit: i128 = 0;
    let mut best_input: u64 = 0;

    // --- LA MODIFICATION EST ICI ---
    // On boucle tant que l'intervalle de recherche est significatif.
    // 1000 lamports est une précision largement suffisante.
    for _ in 0..32 { // On garde une limite de 32 itérations comme sécurité absolue
        if low_bound > high_bound || (high_bound - low_bound) < 1000 {
            break;
        }

        let m1 = low_bound + (high_bound - low_bound) / 3;
        let m2 = high_bound - (high_bound - low_bound) / 3;

        if m1 >= m2 { break; }

        let profit1 = profit_fn(m1).unwrap_or(i128::MIN);
        let profit2 = profit_fn(m2).unwrap_or(i128::MIN);

        if profit1 > max_profit {
            max_profit = profit1;
            best_input = m1;
        }
        if profit2 > max_profit {
            max_profit = profit2;
            best_input = m2;
        }

        if profit1 < profit2 {
            low_bound = m1 + 1; // On explore la partie droite
        } else {
            high_bound = m2 - 1; // On explore la partie gauche
        }
    }
    // --- FIN DE LA MODIFICATION ---

    if max_profit > 0 {
        Some((best_input, max_profit))
    } else {
        None
    }
}