use crate::decoders::{Pool};
use std::collections::HashMap;
use once_cell::sync::Lazy; // Utiliser once_cell pour une initialisation statique propre
use serde::{Serialize, Deserialize};

// Structure pour stocker les coûts mesurés pour chaque type de DEX
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct DexCuCosts {
    pub base_cost: u64,
    pub cost_per_tick: Option<u64>,
}

// NOTRE BASE DE DONNÉES DE COMPUTE UNITS
// Ces valeurs sont des exemples initiaux. Vous devrez les remplacer
// par les résultats de votre propre analyse avec `cu_analyzer`.
static CU_COSTS_DATABASE: Lazy<HashMap<&'static str, DexCuCosts>> = Lazy::new(|| {
    let mut m = HashMap::new();
    // AMMs (coût fixe)
    m.insert("Raydium AMM V4", DexCuCosts { base_cost: 35_000, cost_per_tick: None });
    m.insert("Raydium CPMM", DexCuCosts { base_cost: 48_000, cost_per_tick: None });
    m.insert("Pump AMM", DexCuCosts { base_cost: 25_000, cost_per_tick: None });
    m.insert("Meteora DAMM V1", DexCuCosts { base_cost: 60_000, cost_per_tick: None });

    // CLMMs (coût variable)
    m.insert("Raydium CLMM", DexCuCosts { base_cost: 80_000, cost_per_tick: Some(25_000) });
    m.insert("Orca Whirlpool", DexCuCosts { base_cost: 75_000, cost_per_tick: Some(30_000) });
    m.insert("Meteora DLMM", DexCuCosts { base_cost: 90_000, cost_per_tick: Some(15_000) }); // Les "bins" sont similaires aux ticks
    m.insert("Meteora DAMM V2", DexCuCosts { base_cost: 110_000, cost_per_tick: Some(18_000) });

    m
});

// Helper pour obtenir le nom du type de pool
fn get_pool_type_name(pool: &Pool) -> &'static str {
    match pool {
        Pool::RaydiumAmmV4(_) => "Raydium AMM V4",
        Pool::RaydiumCpmm(_) => "Raydium CPMM",
        Pool::RaydiumClmm(_) => "Raydium CLMM",
        Pool::MeteoraDammV1(_) => "Meteora DAMM V1",
        Pool::MeteoraDammV2(_) => "Meteora DAMM V2",
        Pool::MeteoraDlmm(_) => "Meteora DLMM",
        Pool::OrcaWhirlpool(_) => "Orca Whirlpool",
        Pool::PumpAmm(_) => "Pump AMM",
    }
}

/// Estime le coût en Compute Units pour un swap sur un seul pool.
fn estimate_single_swap(pool: &Pool, ticks_crossed: u32) -> u64 {
    let pool_type = get_pool_type_name(pool);
    if let Some(costs) = CU_COSTS_DATABASE.get(pool_type) {
        let mut total_cost = costs.base_cost;
        if let Some(cost_per_tick) = costs.cost_per_tick {
            total_cost += ticks_crossed as u64 * cost_per_tick;
        }
        total_cost
    } else {
        150_000 // Valeur par défaut sûre si le type est inconnu
    }
}

/// Estime le coût total en CUs pour une transaction d'arbitrage à deux swaps.
/// Estime le coût total en CUs pour une transaction d'arbitrage à deux swaps.
pub fn estimate_arbitrage_cost(
    pool_buy_from: &Pool,
    ticks_crossed_buy: u32,
    pool_sell_to: &Pool,
    ticks_crossed_sell: u32,
) -> u64 {
    let cost_buy = estimate_single_swap(pool_buy_from, ticks_crossed_buy);
    let cost_sell = estimate_single_swap(pool_sell_to, ticks_crossed_sell);

    // Surcharge pour notre programme on-chain `atomic_arb_executor`
    const EXECUTOR_OVERHEAD: u64 = 20_000;

    // Calcul du coût brut estimé
    let raw_estimated_cost = cost_buy + cost_sell + EXECUTOR_OVERHEAD;

    // <-- NOUVEAU : Ajout d'une marge de sécurité de 10%
    const SAFETY_MARGIN_PERCENT: u64 = 10;
    let safety_margin = (raw_estimated_cost * SAFETY_MARGIN_PERCENT) / 100;

    // On retourne le coût brut + la marge de sécurité
    raw_estimated_cost + safety_margin
}