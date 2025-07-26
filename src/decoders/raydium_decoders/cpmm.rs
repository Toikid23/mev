// src/decoders/raydium_decoders/cpmm.rs

use bytemuck::{from_bytes, Pod, Zeroable};
use solana_sdk::pubkey::Pubkey;
use anyhow::{bail, Result};

// Discriminator pour les comptes PoolState du programme CPMM
const CPMM_POOL_STATE_DISCRIMINATOR: [u8; 8] = [247, 237, 227, 245, 215, 195, 222, 70];

// --- STRUCTURE DE SORTIE PROPRE ---
// Contient les infos décodées et utiles du PoolState CPMM.
// Notez que nous extrayons l'adresse de l'AmmConfig pour une lecture ultérieure.
#[derive(Debug, Clone)]
pub struct DecodedCpmmPool {
    pub address: Pubkey,
    pub amm_config: Pubkey,
    pub token_0_mint: Pubkey,
    pub token_1_mint: Pubkey,
    pub token_0_vault: Pubkey,
    pub token_1_vault: Pubkey,
    pub status: u8,
}

// --- STRUCTURE DE DONNÉES BRUTES (Miroir exact de l'IDL) ---
#[repr(C, packed)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
struct CpmmPoolStateData {
    pub amm_config: Pubkey,
    pub pool_creator: Pubkey,
    pub token_0_vault: Pubkey,
    pub token_1_vault: Pubkey,
    pub lp_mint: Pubkey,
    pub token_0_mint: Pubkey,
    pub token_1_mint: Pubkey,
    pub token_0_program: Pubkey,
    pub token_1_program: Pubkey,
    pub observation_key: Pubkey,
    pub auth_bump: u8,
    pub status: u8,
    pub lp_mint_decimals: u8,
    pub mint_0_decimals: u8,
    pub mint_1_decimals: u8,
    pub lp_supply: u64,
    pub protocol_fees_token_0: u64,
    pub protocol_fees_token_1: u64,
    pub fund_fees_token_0: u64,
    pub fund_fees_token_1: u64,
    pub open_time: u64,
    pub recent_epoch: u64,
    pub padding: [u64; 31],
}

/// Tente de décoder les données brutes d'un compte Raydium CPMM PoolState.
pub fn decode_pool(address: &Pubkey, data: &[u8]) -> Result<DecodedCpmmPool> {
    // Étape 1: Vérifier le discriminator
    if data.get(..8) != Some(&CPMM_POOL_STATE_DISCRIMINATOR) {
        bail!("Invalid discriminator. Not a Raydium CPMM PoolState account.");
    }

    let data_slice = &data[8..];

    // Étape 2: Vérifier la taille
    if data_slice.len() != std::mem::size_of::<CpmmPoolStateData>() {
        bail!(
            "CPMM PoolState data length mismatch. Expected {}, got {}.",
            std::mem::size_of::<CpmmPoolStateData>(),
            data_slice.len()
        );
    }

    // Étape 3: "Caster" les données
    let pool_struct: &CpmmPoolStateData = from_bytes(data_slice);

    // Étape 4: Créer la sortie propre et unifiée
    Ok(DecodedCpmmPool {
        address: *address,
        amm_config: pool_struct.amm_config,
        token_0_mint: pool_struct.token_0_mint,
        token_1_mint: pool_struct.token_1_mint,
        token_0_vault: pool_struct.token_0_vault,
        token_1_vault: pool_struct.token_1_vault,
        status: pool_struct.status,
    })
}