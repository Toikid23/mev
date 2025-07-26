// src/decoders/orca_decoders/whirlpool_decoder.rs

use bytemuck::{from_bytes, Pod, Zeroable};
use solana_sdk::pubkey::Pubkey;
use anyhow::{anyhow, Result};
use crate::decoders::DecodedClmmPool;

// --- DÉFINITION DE LA STRUCT DE DÉCODAGE (Miroir de l'IDL) ---
// Cette structure est conçue pour correspondre au layout binaire d'un compte Whirlpool.
// Les 8 premiers bytes de chaque compte Anchor sont un "discriminator", que nous ignorons.
// Nous allons donc créer une struct pour les données qui viennent *après* ces 8 bytes.

#[repr(C, packed)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
pub struct WhirlpoolRewardInfo {
    pub mint: Pubkey,
    pub vault: Pubkey,
    pub authority: Pubkey,
    pub emissions_per_second_x64: u128,
    pub growth_global_x64: u128,
}

#[repr(C, packed)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
pub struct WhirlpoolData {
    // --- Champs que nous avons identifiés dans l'IDL ---
    pub whirlpools_config: Pubkey,
    pub whirlpool_bump: [u8; 1],
    pub tick_spacing: u16,
    pub _padding1: [u8; 2], // Pour l'alignement mémoire
    pub fee_rate: u16,
    pub protocol_fee_rate: u16,
    pub liquidity: u128,
    pub sqrt_price: u128,
    pub tick_current_index: i32,
    pub protocol_fee_owed_a: u64,
    pub protocol_fee_owed_b: u64,
    pub token_mint_a: Pubkey,
    pub token_vault_a: Pubkey,
    pub fee_growth_global_a: u128,
    pub token_mint_b: Pubkey,
    pub token_vault_b: Pubkey,
    pub fee_growth_global_b: u128,
    pub reward_last_updated_timestamp: u64,
    pub reward_infos: [WhirlpoolRewardInfo; 3],
}




/// Tente de décoder les données brutes et les transforme en une structure unifiée.
pub fn decode_pool(address: &Pubkey, data: &[u8]) -> Result<DecodedClmmPool> {
    const DISCRIMINATOR_LENGTH: usize = 8;
    let expected_data_size = std::mem::size_of::<WhirlpoolData>();
    let expected_account_size = DISCRIMINATOR_LENGTH + expected_data_size;

    if data.len() < expected_account_size {
        return Err(anyhow!(
            "Invalid data length for Whirlpool. Expected at least {} bytes, got {}",
            expected_account_size,
            data.len()
        ));
    }

    let pool_data_bytes = &data[DISCRIMINATOR_LENGTH..expected_account_size];
    let whirlpool_data: &WhirlpoolData = from_bytes(pool_data_bytes);

    // --- CALCUL DES FRAIS ---
    // Le fee_rate est un u16 qui représente des "hundredths of a bip" (10^-6)
    // Pour le convertir en pourcentage, il faut diviser par 1 000 000 (pour les bips) et multiplier par 100 (pour le %).
    // Donc, on divise par 10 000.
    let fee_rate_percent = whirlpool_data.fee_rate as f64 / 10000.0;

    // --- CRÉATION DE LA SORTIE PROPRE ---
    Ok(DecodedClmmPool {
        address: *address,
        mint_a: whirlpool_data.token_mint_a,
        mint_b: whirlpool_data.token_mint_b,
        vault_a: whirlpool_data.token_vault_a,
        vault_b: whirlpool_data.token_vault_b,
        sqrt_price: whirlpool_data.sqrt_price,
        tick_current_index: whirlpool_data.tick_current_index,
        fee_rate_percent,
    })
}