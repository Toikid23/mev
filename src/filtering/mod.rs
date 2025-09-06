// DANS : src/filtering/mod.rs

use solana_sdk::pubkey::Pubkey;
use serde::{Serialize, Deserialize};

// On déclare les modules que nous allons créer.
// Pour l'instant, seul `cache` existe, mais les autres viendront.
pub mod cache;
pub mod census;
pub mod watcher;
pub mod engine;

/// Représente l'identité minimale et immuable d'un pool de liquidité.
/// C'est la structure que nous stockerons dans notre cache "univers" sur le disque.
/// Elle est conçue pour être légère et contenir uniquement les informations nécessaires
/// pour identifier un pool et savoir quels comptes surveiller pour détecter une activité.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolIdentity {
    /// L'adresse du compte du pool lui-même.
    pub address: Pubkey,

    /// L'adresse du mint du premier token de la paire.
    pub mint_a: Pubkey,

    /// L'adresse du mint du deuxième token de la paire.
    pub mint_b: Pubkey,

    /// La liste des comptes dont la modification signale une activité sur ce pool.
    /// - Pour les AMM/CPMM : Ce sera les deux vaults (vault_a, vault_b).
    /// - Pour les CLMM/DLMM : Ce sera l'adresse du pool lui-même (où `sqrt_price` change).
    pub accounts_to_watch: Vec<Pubkey>,
}