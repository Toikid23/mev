use crate::decoders::{Pool, PoolOperations};
use crate::graph_engine::Graph;
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::pubkey::Pubkey;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

// Imports nécessaires pour la logique d'escalade.
// On importe les modules pour utiliser leurs fonctions qualifiées (ex: whirlpool::rehydrate_...)
use crate::decoders::meteora::dlmm;
use crate::decoders::orca::whirlpool;

// --- Structs de Données ---

#[derive(Debug, Clone)]
pub struct ArbitrageOpportunity {
    pub profit_in_lamports: u64,
    pub amount_in: u64,
    pub token_in_mint: Pubkey,
    pub token_intermediate_mint: Pubkey,
    pub pool_buy_from_key: Pubkey,
    pub pool_sell_to_key: Pubkey,
}

#[derive(Debug)]
pub enum OptimizationResult {
    Optimal(ArbitrageOpportunity),
    HitTheWall(ArbitrageOpportunity),
}

// --- Fonction Principale (Chef d'Orchestre) ---

pub async fn find_spatial_arbitrage(
    graph: Arc<Mutex<Graph>>,
    rpc_client: Arc<RpcClient>,
) -> Vec<ArbitrageOpportunity> {
    const MINIMUM_PROFIT_LAMPS: u64 = 100_000;

    let mut opportunities = Vec::new();
    let mut pools_map_clone: HashMap<Pubkey, Pool>;
    {
        let graph_guard = graph.lock().await;
        pools_map_clone = graph_guard.pools.clone();
    }

    let mut pools_by_pair: HashMap<(Pubkey, Pubkey), Vec<Pubkey>> = HashMap::new();
    for (pool_key, pool) in &pools_map_clone {
        let (mut mint_a, mut mint_b) = pool.get_mints();
        if mint_a > mint_b { std::mem::swap(&mut mint_a, &mut mint_b); }
        pools_by_pair.entry((mint_a, mint_b)).or_default().push(*pool_key);
    }

    for ((mint_a, mint_b), pool_keys) in pools_by_pair.iter() {
        if pool_keys.len() < 2 { continue; }

        for i in 0..pool_keys.len() {
            for j in (i + 1)..pool_keys.len() {
                let key1 = pool_keys[i];
                let key2 = pool_keys[j];

                let pool1 = pools_map_clone.get(&key1).unwrap().clone();
                let pool2 = pools_map_clone.get(&key2).unwrap().clone();

                let quote1_res = pool1.get_quote(mint_a, 1_000_000, 0);
                let quote2_res = pool2.get_quote(mint_a, 1_000_000, 0);

                if let (Ok(quote1), Ok(quote2)) = (quote1_res, quote2_res) {
                    let (mut pool_buy_from, mut pool_sell_to, buy_key, _sell_key) = if quote1 > quote2 {
                        (pool1, pool2, key1, key2)
                    } else if quote2 > quote1 {
                        (pool2, pool1, key2, key1)
                    } else {
                        continue;
                    };

                    let initial_result = find_optimal_arbitrage(&mut pool_buy_from, &mut pool_sell_to, *mint_a, *mint_b);

                    if let Some(optimization_result) = initial_result {
                        // --- CORRECTION DE L'ERREUR DE TYPE [E0308] ---
                        // On déplace la logique de push et de filtrage après le `match`
                        // pour que toutes les branches retournent une `ArbitrageOpportunity`.
                        let final_opportunity = match optimization_result {
                            OptimizationResult::Optimal(opp) => opp,
                            OptimizationResult::HitTheWall(partial_opp) => {
                                let mut rehydrated_pool = pools_map_clone.get_mut(&buy_key).unwrap().clone();
                                let mut rehydrated_ok = false;

                                let (pool_mint_a, _) = rehydrated_pool.get_mints();
                                let go_up = pool_mint_a == *mint_a;

                                match &mut rehydrated_pool {
                                    Pool::OrcaWhirlpool(p) => {
                                        if whirlpool::rehydrate_for_escalation(p, &rpc_client, go_up).await.is_ok() {
                                            rehydrated_ok = true;
                                        }
                                    },
                                    Pool::MeteoraDlmm(p) => {
                                        if dlmm::rehydrate_for_escalation(p, &rpc_client, go_up).await.is_ok() {
                                            rehydrated_ok = true;
                                        }
                                    },
                                    _ => {}
                                }

                                if rehydrated_ok {
                                    if let Some(final_result) = find_optimal_arbitrage(&mut rehydrated_pool, &mut pool_sell_to, *mint_a, *mint_b) {
                                        let final_opp = match final_result {
                                            OptimizationResult::Optimal(opp) => opp,
                                            OptimizationResult::HitTheWall(opp) => opp,
                                        };
                                        if final_opp.profit_in_lamports > partial_opp.profit_in_lamports {
                                            final_opp
                                        } else {
                                            partial_opp
                                        }
                                    } else {
                                        partial_opp
                                    }
                                } else {
                                    partial_opp
                                }
                            }
                        };
                        // --- FIN DE LA CORRECTION ---

                        if final_opportunity.profit_in_lamports >= MINIMUM_PROFIT_LAMPS {
                            println!(
                                "[OPPORTUNITY] Profit: {} lamports | AmountIn: {} {} | BuyOn: {} | SellOn: {}",
                                final_opportunity.profit_in_lamports,
                                final_opportunity.amount_in,
                                final_opportunity.token_in_mint,
                                final_opportunity.pool_buy_from_key,
                                final_opportunity.pool_sell_to_key
                            );
                            opportunities.push(final_opportunity);
                        }
                    }
                }
            }
        }
    }
    opportunities
}


// --- Fonction d'Optimisation (Calculateur) ---
// (Cette fonction est déjà correcte et n'a pas besoin de changement)
fn find_optimal_arbitrage(
    pool_buy_from: &mut Pool,
    pool_sell_to: &mut Pool,
    token_in_mint: Pubkey,
    token_intermediate_mint: Pubkey,
) -> Option<OptimizationResult> {
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
    let mut hit_the_wall = false;

    for _ in 0..32 {
        let mid_amount_intermediate = low_bound.saturating_add(high_bound) / 2;
        if mid_amount_intermediate == 0 { break; }

        let cost_res = pool_buy_from.get_required_input(&token_intermediate_mint, mid_amount_intermediate, ts);
        let revenue_res = pool_sell_to.get_quote(&token_intermediate_mint, mid_amount_intermediate, ts);

        if let (Ok(cost), Ok(revenue)) = (cost_res, revenue_res) {
            let profit = revenue as i128 - cost as i128;

            if profit > max_profit {
                max_profit = profit;
                best_amount_intermediate = mid_amount_intermediate;
            }
            if profit > 0 {
                low_bound = mid_amount_intermediate;
            } else {
                high_bound = mid_amount_intermediate;
            }
        } else {
            hit_the_wall = true;
            high_bound = mid_amount_intermediate;
        }
    }

    if max_profit > 0 {
        if best_amount_intermediate.saturating_add(1) >= high_bound {
            hit_the_wall = true;
        }

        if let Ok(final_cost) = pool_buy_from.get_required_input(&token_intermediate_mint, best_amount_intermediate, ts) {
            let opportunity = ArbitrageOpportunity {
                profit_in_lamports: max_profit as u64,
                amount_in: final_cost,
                token_in_mint,
                token_intermediate_mint,
                pool_buy_from_key: pool_buy_from.address(),
                pool_sell_to_key: pool_sell_to.address(),
            };
            if hit_the_wall {
                return Some(OptimizationResult::HitTheWall(opportunity));
            } else {
                return Some(OptimizationResult::Optimal(opportunity));
            }
        }
    }

    None
}