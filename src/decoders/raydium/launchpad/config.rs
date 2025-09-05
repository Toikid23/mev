// src/decoders/raydium_decoders/config

use bytemuck::{from_bytes, Pod, Zeroable};
use solana_sdk::pubkey::Pubkey;
use anyhow::{bail, Result};

// Discriminator pour les comptes "GlobalConfig" (trouvé dans l'IDL)
const GLOBAL_CONFIG_DISCRIMINATOR: [u8; 8] = [149, 8, 156, 202, 160, 252, 176, 217];

// --- STRUCTURE DE SORTIE PROPRE ---
// Contient les infos décodées et utiles du GlobalConfig.
#[derive(Debug, Clone)]
pub struct DecodedGlobalConfig {
    pub trade_fee_rate: u64, // Frais de trading (ex: 3000 pour 0.3%)
    pub curve_type: u8,      // Le type de courbe ! 0 = ConstantProduct, 1 = Fixed, 2 = Linear
    pub quote_mint: Pubkey,
}

// --- STRUCTURE DE DONNÉES BRUTES (Miroir de l'IDL) ---
#[repr(C, packed)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
struct GlobalConfigData {
    pub epoch: u64,
    pub curve_type: u8,
    pub index: u16,
    pub migrate_fee: u64,
    pub trade_fee_rate: u64,
    pub max_share_fee_rate: u64,
    pub min_base_supply: u64,
    pub max_lock_rate: u64,
    pub min_base_sell_rate: u64,
    pub min_base_migrate_rate: u64,
    pub min_quote_fund_raising: u64,
    pub quote_mint: Pubkey,
    pub protocol_fee_owner: Pubkey,
    pub migrate_fee_owner: Pubkey,
    pub migrate_to_amm_wallet: Pubkey,
    pub migrate_to_cpswap_wallet: Pubkey,
    pub padding: [u64; 16],
}

/// Tente de décoder les données brutes d'un compte GlobalConfig de Raydium Launchpad.
pub fn decode_global_config(data: &[u8]) -> Result<DecodedGlobalConfig> {
    if data.get(..8) != Some(&GLOBAL_CONFIG_DISCRIMINATOR) {
        bail!("Invalid discriminator. Not a GlobalConfig account.");
    }

    let data_slice = &data[8..];

    if data_slice.len() != size_of::<GlobalConfigData>() {
        bail!(
            "GlobalConfig data length mismatch. Expected {}, got {}.",
            size_of::<GlobalConfigData>(),
            data_slice.len()
        );
    }

    let config_struct: &GlobalConfigData = from_bytes(data_slice);

    Ok(DecodedGlobalConfig {
        trade_fee_rate: config_struct.trade_fee_rate,
        curve_type: config_struct.curve_type,
        quote_mint: config_struct.quote_mint,
    })
}