// src/strategies/mod.rs

// On déclare toutes les stratégies de trading que le bot pourra exécuter.
// Chaque module implémentera la logique pour trouver un type d'opportunité spécifique.
pub mod backrunner;
pub mod copy_trade;
pub mod spfa_arb;       // Notre stratégie principale d'arbitrage cyclique.
pub mod spam_onchain;
pub mod spatial;        // Notre stratégie secondaire d'arbitrage spatial.