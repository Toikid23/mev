use crate::decoders::{Pool, PoolOperations};
use crate::graph_engine::Graph; // On importe notre nouvelle structure Graph
use solana_sdk::pubkey::Pubkey;
use std::collections::HashMap;
use std::sync::Arc;
use anyhow::Result;
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
    // On utilise une unité de base standardisée pour comparer les prix (ex: 0.1 SOL)
    // Cela évite les erreurs sur les tokens à faible liquidité ou avec peu de décimales.
    const PRICE_CHECK_AMOUNT: u64 = 100_000_000; // 0.1 SOL en lamports (ou l'équivalent pour d'autres tokens)

    let mut opportunities = Vec::new();

    let pools_by_pair = {
        let pools_map_reader = graph.pools.read().await;
        let mut map: HashMap<(Pubkey, Pubkey), Vec<Pubkey>> = HashMap::new();
        for (pool_key, pool_arc_rwlock) in pools_map_reader.iter() {
            let pool_reader = pool_arc_rwlock.read().await;
            let (mut mint_a, mut mint_b) = pool_reader.get_mints();
            if mint_a > mint_b { std::mem::swap(&mut mint_a, &mut mint_b); }
            map.entry((mint_a, mint_b)).or_default().push(*pool_key);
        }
        map
    };

    // --- DÉBUT DE LA NOUVELLE LOGIQUE O(n) ---
    for ((mint_a, mint_b), pool_keys) in pools_by_pair.iter() {
        if pool_keys.len() < 2 { continue; }

        let mut best_seller: Option<(u64, Pubkey)> = None; // (coût pour acheter B, clé du pool)
        let mut best_buyer: Option<(u64, Pubkey)> = None;  // (revenu en vendant B, clé du pool)

        // Boucle unique pour trouver les meilleurs prix (O(n))
        for pool_key in pool_keys {
            let pool_arc_rwlock = {
                let pools_reader = graph.pools.read().await;
                match pools_reader.get(pool_key) {
                    Some(p) => p.clone(),
                    None => continue,
                }
            };

            let pool_data = pool_arc_rwlock.read().await;

            // Calculer le "prix de vente" du pool (coût pour acheter mint_b avec mint_a)
            if let Ok(cost_to_buy_b) = pool_data.get_quote(mint_a, PRICE_CHECK_AMOUNT, 0) {
                if cost_to_buy_b > 0 {
                    if best_seller.is_none() || cost_to_buy_b < best_seller.unwrap().0 {
                        best_seller = Some((cost_to_buy_b, *pool_key));
                    }
                }
            }

            // Calculer le "prix d'achat" du pool (revenu en vendant mint_b pour mint_a)
            if let Ok(revenue_from_selling_b) = pool_data.get_quote(mint_b, PRICE_CHECK_AMOUNT, 0) {
                if best_buyer.is_none() || revenue_from_selling_b > best_buyer.unwrap().0 {
                    best_buyer = Some((revenue_from_selling_b, *pool_key));
                }
            }
        }

        // Si on a trouvé un acheteur et un vendeur, et que le prix de vente est inférieur au prix d'achat
        if let (Some((_sell_price, sell_key)), Some((buy_price, buy_key))) = (best_seller, best_buyer) {
            if sell_key != buy_key && buy_price > PRICE_CHECK_AMOUNT {
                // Opportunité détectée ! On lance maintenant l'optimiseur de profit.
                let (mut pool_buy_from, mut pool_sell_to) = {
                    let pools_reader = graph.pools.read().await;
                    let p_buy_arc = pools_reader.get(&sell_key).unwrap().clone();
                    let p_sell_arc = pools_reader.get(&buy_key).unwrap().clone();
                    drop(pools_reader);
                    let (p_buy_guard, p_sell_guard) = tokio::join!(p_buy_arc.read(), p_sell_arc.read());
                    (p_buy_guard.clone(), p_sell_guard.clone())
                };

                // On appelle la fonction d'optimisation UNE SEULE FOIS.
                if let Some(final_opportunity) = find_optimal_arbitrage(&mut pool_buy_from, &mut pool_sell_to, *mint_a, *mint_b) {
                    if final_opportunity.profit_in_lamports >= MINIMUM_PROFIT_LAMPS {
                        opportunities.push(final_opportunity);
                    }
                }
            }
        }
    }
    // --- FIN DE LA NOUVELLE LOGIQUE ---

    opportunities
}

fn find_optimal_arbitrage(
    pool_buy_from: &mut Pool,
    pool_sell_to: &mut Pool,
    token_in_mint: Pubkey,
    token_intermediate_mint: Pubkey,
) -> Option<ArbitrageOpportunity> {
    let ts = 0;

    let (buy_res_a, buy_res_b) = pool_buy_from.get_reserves();
    let (sell_res_a, sell_res_b) = pool_sell_to.get_reserves();
    let (buy_from_intermediate_reserve, _) = if pool_buy_from.get_mints().0 == token_intermediate_mint { (buy_res_a, buy_res_b) } else { (buy_res_b, buy_res_a) };
    let (sell_to_intermediate_reserve, _) = if pool_sell_to.get_mints().0 == token_intermediate_mint { (sell_res_a, sell_res_b) } else { (sell_res_b, sell_res_a) };

    let low_bound: u64 = 0;
    let mut high_bound: u64 = std::cmp::min(buy_from_intermediate_reserve, sell_to_intermediate_reserve);
    if high_bound == 0 { high_bound = 500 * 1_000_000_000; }

    // CORRIGÉ : La closure retourne maintenant un `anyhow::Result<i128>`.
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

/// Optimise une fonction de profit unimodale en utilisant la recherche ternaire.
///
/// # Arguments
/// * `low_bound` - La borne inférieure de la recherche (ex: 0).
/// * `high_bound` - La borne supérieure de la recherche (ex: la liquidité minimale).
/// * `profit_fn` - Une closure qui prend un `u64` (montant) et retourne le profit en `i128`.
///
/// # Retours
/// `None` si aucun profit n'est trouvé.
/// `Some((best_input, max_profit))` avec le montant d'entrée optimal et le profit correspondant.
/// Optimise une fonction de profit unimodale en utilisant la recherche ternaire.
fn ternary_search_optimizer<F>(
    mut low_bound: u64,
    mut high_bound: u64,
    mut profit_fn: F,
) -> Option<(u64, i128)>
where
// CORRIGÉ : On utilise `anyhow::Result` qui inclut le type d'erreur.
    F: FnMut(u64) -> Result<i128>,
{
    let mut max_profit: i128 = 0;
    let mut best_input: u64 = 0;

    for _ in 0..100 {
        if low_bound > high_bound {
            break;
        }

        let m1 = low_bound + (high_bound - low_bound) / 3;
        let m2 = high_bound - (high_bound - low_bound) / 3;

        if m1 >= m2 { break; }

        let profit1 = match profit_fn(m1) {
            Ok(p) => p,
            Err(_) => i128::MIN,
        };
        let profit2 = match profit_fn(m2) {
            Ok(p) => p,
            Err(_) => i128::MIN,
        };

        if profit1 > max_profit {
            max_profit = profit1;
            best_input = m1;
        }
        if profit2 > max_profit {
            max_profit = profit2;
            best_input = m2;
        }

        if profit1 < profit2 {
            low_bound = m1;
        } else {
            high_bound = m2;
        }
    }

    if max_profit > 0 {
        Some((best_input, max_profit))
    } else {
        None
    }
}