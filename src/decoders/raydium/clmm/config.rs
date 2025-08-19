// src/decoders/raydium_decoders/config

use bytemuck::{from_bytes, Pod, Zeroable};
use solana_sdk::pubkey::Pubkey;
use anyhow::{bail, Result};

// Discriminator pour les comptes AmmConfig (partagé avec CPMM)
const AMM_CONFIG_DISCRIMINATOR: [u8; 8] = [218, 244, 33, 104, 203, 203, 43, 111];

#[derive(Debug, Clone)]
pub struct DecodedClmmConfig {
    pub trade_fee_rate: u32,
    pub protocol_fee_rate: u32,
    pub tick_spacing: u16,
}

#[repr(C, packed)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
struct ClmmAmmConfigData {
    pub bump: u8,
    // PAS de padding ici
    pub index: u16,
    pub owner: Pubkey,
    pub protocol_fee_rate: u32,
    pub trade_fee_rate: u32,
    pub tick_spacing: u16,
    pub fund_fee_rate: u32,
    pub padding_u32: u32,
    pub fund_owner: Pubkey,
    pub padding: [u64; 3],
}

/// Décode un compte AmmConfig de Raydium CLMM.
pub fn decode_config(data: &[u8]) -> Result<DecodedClmmConfig> {
    if data.get(..8) != Some(&AMM_CONFIG_DISCRIMINATOR) {
        bail!("Invalid discriminator. Not a Raydium CLMM AmmConfig account.");
    }

    let data_slice = &data[8..];
    if data_slice.len() != std::mem::size_of::<ClmmAmmConfigData>() {
        bail!(
            "CLMM AmmConfig data length mismatch. Expected {}, got {}.",
            std::mem::size_of::<ClmmAmmConfigData>(),
            data_slice.len()
        );
    }

    let config_struct: &ClmmAmmConfigData = from_bytes(data_slice);

    Ok(DecodedClmmConfig {
        trade_fee_rate: config_struct.trade_fee_rate,
        protocol_fee_rate: config_struct.protocol_fee_rate,
        tick_spacing: config_struct.tick_spacing,
    })
}