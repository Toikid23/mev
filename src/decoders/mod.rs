
use solana_sdk::pubkey::Pubkey;
use anyhow::{Result, anyhow};

pub mod orca_decoders;
pub mod raydium_decoders;
pub mod spl_token_decoders;
pub mod meteora_decoders;
// src/decoders/mod.rs














// --- STRUCTURES DE SORTIE UNIFIÉES ---

// Représente les informations essentielles d'un pool AMM classique (x * y = k)
#[derive(Debug, Clone)]
pub struct DecodedAmmPool {
    pub address: Pubkey,
    pub mint_a: Pubkey,
    pub mint_b: Pubkey,
    pub vault_a: Pubkey,
    pub vault_b: Pubkey,
    pub total_fee_percent: f64,
}



#[derive(Debug, Clone)]
pub struct DecodedClmmPool {
    pub address: Pubkey,
    pub mint_a: Pubkey,
    pub mint_b: Pubkey,
    pub vault_a: Pubkey,
    pub vault_b: Pubkey,

    // Le prix n'est pas direct, il est stocké sous forme de racine carrée
    pub sqrt_price: u128,
    // La "case" de prix actuelle
    pub tick_current_index: i32,

    // Les frais sont plus simples sur les Whirlpools, il y a un taux de base.
    pub fee_rate_percent: f64,
}



#[derive(Debug, Clone)]
pub struct DecodedLaunchpadPool {
    pub address: Pubkey,
    pub mint_a: Pubkey, // Le "base_mint" dans le jargon Launchpad
    pub mint_b: Pubkey, // Le "quote_mint"
    pub vault_a: Pubkey,
    pub vault_b: Pubkey,

    // Un Launchpad n'a pas de "frais" au sens d'un AMM.
    // Les informations importantes sont liées à l'état de la vente.
    pub total_base_sold: u64,
    pub total_quote_raised: u64,
    pub global_config: Pubkey, // Pour connaître le type de courbe et les frais globaux
    pub virtual_base: u64,
    pub virtual_quote: u64,
}


// DANS: src/decoders/mod.rs

// ... (après DecodedLaunchpadPool)

#[derive(Debug, Clone)]
pub struct DecodedStablePool {
    pub address: Pubkey,
    pub mint_a: Pubkey,
    pub mint_b: Pubkey,
    pub vault_a: Pubkey,
    pub vault_b: Pubkey,

    // Le paramètre clé de la courbe de prix pour les stable swaps
    pub amp: u64,

    // Les frais sont directement dans le pool
    pub total_fee_percent: f64,
}