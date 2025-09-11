// DANS : src/strategies/spatial.rs

use crate::decoders::{Pool, PoolOperations};
use crate::graph_engine::Graph;
use solana_sdk::pubkey::Pubkey;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

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
    graph: Arc<Mutex<Graph>>,
) -> Vec<ArbitrageOpportunity> {
    const MINIMUM_PROFIT_LAMPS: u64 = 100_000;
    let mut opportunities = Vec::new();

    let graph_guard = graph.lock().await;
    let pools_map_ref: &HashMap<Pubkey, Arc<Pool>> = &graph_guard.pools;

    let mut pools_by_pair: HashMap<(Pubkey, Pubkey), Vec<Pubkey>> = HashMap::new();
    for (pool_key, pool_arc) in pools_map_ref {
        let (mut mint_a, mut mint_b) = pool_arc.get_mints();
        if mint_a > mint_b { std::mem::swap(&mut mint_a, &mut mint_b); }
        pools_by_pair.entry((mint_a, mint_b)).or_default().push(*pool_key);
    }
    drop(graph_guard);

    for ((mint_a, mint_b), pool_keys) in pools_by_pair.iter() {
        if pool_keys.len() < 2 { continue; }

        for i in 0..pool_keys.len() {
            for j in (i + 1)..pool_keys.len() {
                let key1 = pool_keys[i];
                let key2 = pool_keys[j];

                let graph_guard = graph.lock().await;
                let pool1_arc = match graph_guard.pools.get(&key1) { Some(p) => p.clone(), None => continue };
                let pool2_arc = match graph_guard.pools.get(&key2) { Some(p) => p.clone(), None => continue };
                drop(graph_guard);

                // CORRIGÉ : `mut` a été retiré, car non nécessaire.
                let pool1 = (*pool1_arc).clone();
                let pool2 = (*pool2_arc).clone();

                let quote1_res = pool1.get_quote(mint_a, 1_000_000, 0);
                let quote2_res = pool2.get_quote(mint_a, 1_000_000, 0);

                if let (Ok(quote1), Ok(quote2)) = (quote1_res, quote2_res) {
                    let (mut pool_buy_from, mut pool_sell_to, _buy_key, _sell_key) = if quote1 > quote2 {
                        (pool1, pool2, key1, key2)
                    } else if quote2 > quote1 {
                        (pool2, pool1, key2, key1)
                    } else {
                        continue;
                    };

                    if let Some(final_opportunity) = find_optimal_arbitrage(&mut pool_buy_from, &mut pool_sell_to, *mint_a, *mint_b) {
                        if final_opportunity.profit_in_lamports >= MINIMUM_PROFIT_LAMPS {
                            opportunities.push(final_opportunity);
                        }
                    }
                }
            }
        }
    }
    opportunities
}

fn find_optimal_arbitrage(
    pool_buy_from: &mut Pool,
    pool_sell_to: &mut Pool,
    token_in_mint: Pubkey,
    token_intermediate_mint: Pubkey,
) -> Option<ArbitrageOpportunity> {
    let ts = 0;
    let mut low_bound: u64 = 0;

    let (buy_res_a, buy_res_b) = pool_buy_from.get_reserves();
    let (sell_res_a, sell_res_b) = pool_sell_to.get_reserves();
    let (buy_from_intermediate_reserve, _) = if pool_buy_from.get_mints().0 == token_intermediate_mint { (buy_res_a, buy_res_b) } else { (buy_res_b, buy_res_a) };
    let (sell_to_intermediate_reserve, _) = if pool_sell_to.get_mints().0 == token_intermediate_mint { (sell_res_a, sell_res_b) } else { (sell_res_b, sell_res_a) };

    let mut high_bound: u64 = std::cmp::min(buy_from_intermediate_reserve, sell_to_intermediate_reserve);
    if high_bound == 0 { high_bound = 500 * 1_000_000_000; }

    let mut best_amount_intermediate: u64 = 0;
    let mut max_profit: i128 = 0;

    let mut consecutive_no_improvement = 0;
    const EARLY_EXIT_THRESHOLD: u8 = 5;

    for _ in 0..32 {
        if low_bound > high_bound { break; }
        let mid_amount_intermediate = low_bound.saturating_add(high_bound) / 2;
        if mid_amount_intermediate == 0 { break; }

        let cost_res = pool_buy_from.get_required_input(&token_intermediate_mint, mid_amount_intermediate, ts);
        let revenue_res = pool_sell_to.get_quote(&token_intermediate_mint, mid_amount_intermediate, ts);

        if let (Ok(cost), Ok(revenue)) = (cost_res, revenue_res) {
            let profit = revenue as i128 - cost as i128;
            let previous_max_profit = max_profit;

            if profit > max_profit {
                max_profit = profit;
                best_amount_intermediate = mid_amount_intermediate;
            }
            if profit > 0 { low_bound = mid_amount_intermediate.saturating_add(1); } else { high_bound = mid_amount_intermediate.saturating_sub(1); }
            if max_profit > previous_max_profit { consecutive_no_improvement = 0; } else { consecutive_no_improvement += 1; }
            if consecutive_no_improvement >= EARLY_EXIT_THRESHOLD { break; }
        } else {
            high_bound = mid_amount_intermediate.saturating_sub(1);
        }
    }

    if max_profit > 0 {
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