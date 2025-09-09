use crate::decoders::{Pool, PoolOperations};
use crate::graph_engine::Graph;
use solana_sdk::pubkey::Pubkey;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use crate::decoders::meteora::dlmm;
use crate::decoders::orca::whirlpool;
use crate::rpc::ResilientRpcClient;

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
    rpc_client: Arc<ResilientRpcClient>,
) -> Vec<ArbitrageOpportunity> {
    const MINIMUM_PROFIT_LAMPS: u64 = 100_000;

    let mut opportunities = Vec::new();

    // --- DÉBUT DE LA MODIFICATION MAJEURE ---
    // On ne clone plus toute la map. On prend un verrou et on travaille sur des références.
    // C'est beaucoup plus rapide et consomme moins de mémoire.
    let graph_guard = graph.lock().await;
    let pools_map_ref: &HashMap<Pubkey, Arc<Pool>> = &graph_guard.pools;

    // `pools_map_clone` n'existe plus, on utilise `pools_map_ref`.
    let mut pools_by_pair: HashMap<(Pubkey, Pubkey), Vec<Pubkey>> = HashMap::new();
    for (pool_key, pool_arc) in pools_map_ref {
        let (mut mint_a, mut mint_b) = pool_arc.get_mints();
        if mint_a > mint_b { std::mem::swap(&mut mint_a, &mut mint_b); }
        pools_by_pair.entry((mint_a, mint_b)).or_default().push(*pool_key);
    }
    // --- FIN DE LA MODIFICATION MAJEURE ---

    for ((mint_a, mint_b), pool_keys) in pools_by_pair.iter() {
        if pool_keys.len() < 2 { continue; }

        for i in 0..pool_keys.len() {
            for j in (i + 1)..pool_keys.len() {
                let key1 = pool_keys[i];
                let key2 = pool_keys[j];

                // --- MODIFICATION ---
                // On obtient des pointeurs Arc (très rapide) au lieu de cloner depuis une map clonée.
                let pool1_arc = pools_map_ref.get(&key1).unwrap();
                let pool2_arc = pools_map_ref.get(&key2).unwrap();

                // On clone localement les deux pools dont on a besoin. C'est une petite copie, pas une grosse.
                // On déréférence le Arc (`**pool_arc`) pour accéder au `Pool` et appeler `.clone()`.
                let pool1 = (**pool1_arc).clone();
                let pool2 = (**pool2_arc).clone();
                // --- FIN DE LA MODIFICATION ---

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
                        let final_opportunity = match optimization_result {
                            OptimizationResult::Optimal(opp) => opp,
                            OptimizationResult::HitTheWall(partial_opp) => {
                                // --- MODIFICATION ---
                                // On récupère le Arc du pool à réhydrater et on le clone.
                                let mut rehydrated_pool = (**pools_map_ref.get(&buy_key).unwrap()).clone();
                                let mut rehydrated_ok = false;
                                // --- FIN DE LA MODIFICATION ---

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

    // --- AMÉLIORATION ICI ---
    let mut consecutive_no_improvement = 0;
    const EARLY_EXIT_THRESHOLD: u8 = 5; // On sort si le profit ne s'améliore pas pendant 5 itérations

    for _ in 0..32 {
        if low_bound > high_bound { // Condition de fin naturelle
            break;
        }

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

            if profit > 0 {
                low_bound = mid_amount_intermediate.saturating_add(1);
            } else {
                high_bound = mid_amount_intermediate.saturating_sub(1);
            }

            // Logique de sortie anticipée
            if max_profit > previous_max_profit {
                consecutive_no_improvement = 0; // Le profit a augmenté, on réinitialise le compteur
            } else {
                consecutive_no_improvement += 1;
            }

            if consecutive_no_improvement >= EARLY_EXIT_THRESHOLD {
                // println!("[Optimizer] Sortie anticipée après {} itérations sans amélioration.", EARLY_EXIT_THRESHOLD);
                break;
            }

        } else {
            hit_the_wall = true;
            high_bound = mid_amount_intermediate.saturating_sub(1);
        }
    }

    // Le reste de la fonction est inchangé
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
            return if hit_the_wall {
                Some(OptimizationResult::HitTheWall(opportunity))
            } else {
                Some(OptimizationResult::Optimal(opportunity))
            }
        }
    }

    None
}