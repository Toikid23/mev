use crate::decoders::{Pool, PoolOperations};
use crate::graph_engine::Graph;
use solana_client::nonblocking::rpc_client::RpcClient;
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
    rpc_client: Arc<RpcClient>,
) -> Vec<ArbitrageOpportunity> {
    let mut opportunities = Vec::new();
    let pools_map_clone: HashMap<Pubkey, Pool>;
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

                // On clone les pools pour chaque test d'arbitrage
                let mut pool1 = pools_map_clone.get(&key1).unwrap().clone();
                let mut pool2 = pools_map_clone.get(&key2).unwrap().clone();

                let test_amount_b = 1_000_000; // 1 USDC

                if let Some(opp) = check_arbitrage(&mut pool1, &mut pool2, *mint_b, *mint_a, test_amount_b, &rpc_client).await {
                    opportunities.push(opp);
                }
                if let Some(opp) = check_arbitrage(&mut pool2, &mut pool1, *mint_b, *mint_a, test_amount_b, &rpc_client).await {
                    opportunities.push(opp);
                }
            }
        }
    }
    opportunities
}

async fn check_arbitrage(
    pool_buy_from: &mut Pool,
    pool_sell_to: &mut Pool,
    token_in_mint: Pubkey,
    token_intermediate_mint: Pubkey,
    amount_in: u64,
    rpc_client: &RpcClient,
) -> Option<ArbitrageOpportunity> {
    // On ignore les pools pour lesquels la logique async n'est pas prête
    // --- LA CORRECTION EST SUR CETTE LIGNE ---
    match (&*pool_buy_from, &*pool_sell_to) {
        // --- FIN DE LA CORRECTION ---
        (Pool::RaydiumClmm(_), _) | (_, Pool::RaydiumClmm(_)) => { return None; }
        _ => {}
    }

    if let Ok(amount_out_intermediate) = pool_buy_from.get_quote_async(&token_in_mint, amount_in, rpc_client).await {
        if amount_out_intermediate == 0 { return None; }

        if let Ok(final_amount_out) = pool_sell_to.get_quote_async(&token_intermediate_mint, amount_out_intermediate, rpc_client).await {
            if final_amount_out > amount_in {
                let profit = final_amount_out - amount_in;

                return Some(ArbitrageOpportunity {
                    profit_in_lamports: profit,
                    amount_in,
                    token_in_mint,
                    token_intermediate_mint,
                    pool_buy_from_key: pool_buy_from.address(),
                    pool_sell_to_key: pool_sell_to.address(),
                });
            }
        }
    }
    None
}