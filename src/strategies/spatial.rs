// src/strategies/spatial.rs

use crate::decoders::{Pool, PoolOperations};
use crate::graph_engine::Graph;
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::pubkey::Pubkey;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex; // On utilise le Mutex de Tokio qui est async-friendly

/// Cherche des opportunités d'arbitrage spatial dans le graphe fourni.
pub async fn find_spatial_arbitrage(graph: Arc<Mutex<Graph>>, rpc_client: Arc<RpcClient>) {
    let pools_map: HashMap<Pubkey, Pool>;
    {
        let graph_guard = graph.lock().await;
        pools_map = graph_guard.pools.clone();
    }

    let mut pools_by_pair: HashMap<(Pubkey, Pubkey), Vec<Pubkey>> = HashMap::new();
    for (pool_key, pool) in &pools_map {
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

                // On clone les deux pools qu'on va analyser.
                // C'est la solution la plus simple pour éviter les problèmes d'emprunt.
                let mut pool1 = pools_map.get(&key1).unwrap().clone();
                let mut pool2 = pools_map.get(&key2).unwrap().clone();

                let test_amount_b = 1_000_000; // 1 USDC

                check_arbitrage(&mut pool1, &mut pool2, *mint_b, *mint_a, test_amount_b, &rpc_client).await;
                check_arbitrage(&mut pool2, &mut pool1, *mint_b, *mint_a, test_amount_b, &rpc_client).await;
            }
        }
    }
}

async fn check_arbitrage(
    pool_buy_from: &mut Pool,
    pool_sell_to: &mut Pool,
    token_in_mint: Pubkey,
    token_out_mint: Pubkey,
    amount_in: u64,
    rpc_client: &RpcClient,
) {
    // --- DÉBUT DE LA CORRECTION ---
    // On dé-référence puis ré-emprunte pour ne pas "déplacer" les &mut.
    match (&*pool_buy_from, &*pool_sell_to) {
        // --- FIN DE LA CORRECTION ---
        (Pool::RaydiumClmm(_), _) | (_, Pool::RaydiumClmm(_)) |
        (Pool::MeteoraDlmm(_), _) | (_, Pool::MeteoraDlmm(_)) => {
            return;
        }
        _ => {}
    }

    if let Ok(amount_out_intermediate) = pool_buy_from.get_quote_async(&token_in_mint, amount_in, rpc_client).await {
        if amount_out_intermediate == 0 { return; }

        if let Ok(final_amount_out) = pool_sell_to.get_quote_async(&token_out_mint, amount_out_intermediate, rpc_client).await {
            if final_amount_out > amount_in {
                let profit = final_amount_out - amount_in;
                println!("\n!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!");
                println!("!!! OPPORTUNITÉ D'ARBITRAGE SPATIAL DÉTECTÉE !!!");
                println!("!!! PROFIT BRUT: {} {}", profit, token_in_mint);
                println!("!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!");
            }
        }
    }
}