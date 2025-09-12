use crate::decoders::{Pool, PoolOperations};
use crate::graph_engine::Graph; // On importe notre nouvelle structure Graph
use solana_sdk::pubkey::Pubkey;
use std::collections::HashMap;
use std::sync::Arc;
// On a seulement besoin de tokio::sync::RwLock pour les types, mais pas d'import direct si on n'utilise pas le nom court.
// On va le retirer pour éviter le warning, car les types sont déjà dans la définition de `Graph`.

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
) -> Vec<ArbitrageOpportunity> {
    const MINIMUM_PROFIT_LAMPS: u64 = 100_000;
    let mut opportunities = Vec::new();

    // Étape 1 : Construire une map locale des paires de tokens.
    // C'est ici qu'on utilise la nouvelle logique pour la première fois.
    let pools_by_pair = {
        // On prend un VERROU EN LECTURE sur la HashMap des pools.
        // Plusieurs stratégies peuvent faire cela en même temps.
        let pools_map_reader = graph.pools.read().await;

        // On aide le compilateur en spécifiant le type complet de notre map locale.
        let mut map: HashMap<(Pubkey, Pubkey), Vec<Pubkey>> = HashMap::new();

        for (pool_key, pool_arc_rwlock) in pools_map_reader.iter() {
            // Pour chaque pool dans la grande map, on prend un VERROU EN LECTURE individuel.
            let pool_reader = pool_arc_rwlock.read().await;
            let (mut mint_a, mut mint_b) = pool_reader.get_mints();
            if mint_a > mint_b { std::mem::swap(&mut mint_a, &mut mint_b); }
            map.entry((mint_a, mint_b)).or_default().push(*pool_key);
        }
        map // Le verrou `pools_map_reader` est automatiquement relâché à la fin de ce bloc.
    };

    // Étape 2 : Itérer sur les paires qui ont au moins deux pools.
    for ((mint_a, mint_b), pool_keys) in pools_by_pair.iter() {
        if pool_keys.len() < 2 { continue; }

        for i in 0..pool_keys.len() {
            for j in (i + 1)..pool_keys.len() {
                let key1 = pool_keys[i];
                let key2 = pool_keys[j];

                // Étape 3 : Récupérer les données clonées des deux pools à comparer.
                let (pool1, pool2) = {
                    // On reprend un VERROU EN LECTURE sur la HashMap pour trouver les Arcs.
                    let pools_reader = graph.pools.read().await;
                    let pool1_arc_rwlock = match pools_reader.get(&key1) { Some(p) => p.clone(), None => continue };
                    let pool2_arc_rwlock = match pools_reader.get(&key2) { Some(p) => p.clone(), None => continue };
                    drop(pools_reader); // On relâche le verrou de la map le plus tôt possible.

                    // On prend les DEUX verrous de lecture individuels en parallèle.
                    let (p1_guard, p2_guard) = tokio::join!(pool1_arc_rwlock.read(), pool2_arc_rwlock.read());

                    // On clone les données pour travailler dessus. Les verrous sont relâchés ici.
                    (p1_guard.clone(), p2_guard.clone())
                };

                // Étape 4 : La logique de calcul reste la même.
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
    // Le corps de cette fonction est correct et reste inchangé.
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