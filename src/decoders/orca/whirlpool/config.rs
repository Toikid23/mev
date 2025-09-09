// DANS: src/decoders/orca/whirlpool/config.rs

use bytemuck::{from_bytes, Pod, Zeroable};
use solana_sdk::pubkey::Pubkey;
use anyhow::{bail, Result};
use serde::{Serialize, Deserialize};
use std::mem::size_of; // <-- Gardez cet import

// --- LA STRUCTURE CORRIGÉE ---
#[derive(Debug, Clone, Copy, Pod, Zeroable, Serialize, Deserialize)]
#[repr(C, packed)]
pub struct DecodedWhirlpoolsConfig {
    pub fee_authority: Pubkey,
    pub collect_protocol_fees_authority: Pubkey,
    pub reward_emissions_super_authority: Pubkey,
    pub default_protocol_fee_rate: u16,
    pub padding: [u8; 2], // <-- AJOUT DU PADDING MANQUANT
}

// --- LA FONCTION NETTOYÉE ET DÉFINITIVE ---
pub fn decode_config(data: &[u8]) -> Result<DecodedWhirlpoolsConfig> {
    const DISCRIMINATOR: [u8; 8] = [157, 20, 49, 224, 217, 87, 193, 254];
    if data.get(..8) != Some(&DISCRIMINATOR) {
        bail!("Invalid discriminator. Not a WhirlpoolsConfig account.");
    }

    let data_slice = &data[8..];
    let expected_size = size_of::<DecodedWhirlpoolsConfig>();

    // Maintenant, cette vérification devrait passer parfaitement.
    if data_slice.len() != expected_size {
        bail!(
            "WhirlpoolsConfig data length mismatch. Expected {}, got {}.",
            expected_size,
            data_slice.len()
        );
    }

    let config_struct: &DecodedWhirlpoolsConfig = from_bytes(data_slice);
    Ok(*config_struct)
}