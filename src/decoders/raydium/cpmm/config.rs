// src/decoders/raydium_decoders/config

use bytemuck::{from_bytes, Pod, Zeroable};
use solana_sdk::pubkey::Pubkey;
use anyhow::{bail, Result};

// Discriminator pour les comptes AmmConfig du programme CPMM
const AMM_CONFIG_DISCRIMINATOR: [u8; 8] = [218, 244, 33, 104, 203, 203, 43, 111];

// --- STRUCTURE DE SORTIE PROPRE ---
// Contient les frais, qui sont l'information la plus critique pour nous.
#[derive(Debug, Clone)]
pub struct DecodedAmmConfig {
    pub trade_fee_rate: u64,
    pub protocol_fee_rate: u64,
    pub fund_fee_rate: u64,
}

// --- STRUCTURE DE DONNÉES BRUTES (Miroir exact de l'IDL) ---
#[repr(C, packed)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
struct AmmConfigData {
    pub bump: u8,
    pub disable_create_pool: u8, // bool est 1 byte
    pub index: u16,
    pub trade_fee_rate: u64,
    pub protocol_fee_rate: u64,
    pub fund_fee_rate: u64,
    pub create_pool_fee: u64,
    pub protocol_owner: Pubkey,
    pub fund_owner: Pubkey,
    pub padding: [u64; 16],
}

/// Tente de décoder les données brutes d'un compte Raydium AmmConfig.
pub fn decode_config(data: &[u8]) -> Result<DecodedAmmConfig> {
    // Étape 1: Vérifier le discriminator
    if data.get(..8) != Some(&AMM_CONFIG_DISCRIMINATOR) {
        bail!("Invalid discriminator. Not a Raydium AmmConfig account.");
    }

    let data_slice = &data[8..];

    // Étape 2: Vérifier la taille
    if data_slice.len() != std::mem::size_of::<AmmConfigData>() {
        bail!(
            "AmmConfig data length mismatch. Expected {}, got {}.",
            std::mem::size_of::<AmmConfigData>(),
            data_slice.len()
        );
    }

    // Étape 3: "Caster" les données
    let config_struct: &AmmConfigData = from_bytes(data_slice);

    // Étape 4: Créer la sortie propre et unifiée
    Ok(DecodedAmmConfig {
        trade_fee_rate: config_struct.trade_fee_rate,
        protocol_fee_rate: config_struct.protocol_fee_rate,
        fund_fee_rate: config_struct.fund_fee_rate,
    })
}