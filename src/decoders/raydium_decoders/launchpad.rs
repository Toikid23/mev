// src/decoders/raydium_decoders/launchpad.rs

use bytemuck::{from_bytes, Pod, Zeroable};
use solana_sdk::pubkey::Pubkey;
use anyhow::{anyhow, Result};
use crate::decoders::DecodedLaunchpadPool;

// --- DÉFINITION DE LA STRUCT DE DÉCODAGE (Miroir de l'IDL "PoolState") ---
// Le programme Launchpad est basé sur Anchor, donc cette structure représente
// les données du compte APRÈS les 8 bytes du discriminator.

#[repr(C, packed)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
pub struct VestingSchedule {
    pub total_locked_amount: u64,
    pub cliff_period: u64,
    pub unlock_period: u64,
    pub start_time: u64,
    pub allocated_share_amount: u64,
}

#[repr(C, packed)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
pub struct PoolStateData {
    pub epoch: u64,
    pub auth_bump: u8,
    pub status: u8,
    pub base_decimals: u8,
    pub quote_decimals: u8,
    pub migrate_type: u8,
    pub supply: u64,
    pub total_base_sell: u64,
    pub virtual_base: u64,
    pub virtual_quote: u64,
    pub real_base: u64,
    pub real_quote: u64,
    pub total_quote_fund_raising: u64,
    pub quote_protocol_fee: u64,
    pub platform_fee: u64,
    pub migrate_fee: u64,
    pub vesting_schedule: VestingSchedule,
    pub global_config: Pubkey,
    pub platform_config: Pubkey,
    pub base_mint: Pubkey,
    pub quote_mint: Pubkey,
    pub base_vault: Pubkey,
    pub quote_vault: Pubkey,
    pub creator: Pubkey,
    pub padding_for_future: [u64; 8],
}




/// Tente de décoder les données brutes et les transforme en une structure unifiée.
pub fn decode_pool(address: &Pubkey, data: &[u8]) -> Result<DecodedLaunchpadPool> {
    const DISCRIMINATOR_LENGTH: usize = 8;
    const POOL_STATE_DISCRIMINATOR: [u8; 8] = [247, 237, 227, 245, 215, 195, 222, 70];

    if data.len() < DISCRIMINATOR_LENGTH {
        return Err(anyhow!("Invalid data: not long enough for a discriminator."));
    }

    if &data[..DISCRIMINATOR_LENGTH] != POOL_STATE_DISCRIMINATOR {
        return Err(anyhow!(
            "Invalid discriminator. Expected {:?}, got {:?}",
            POOL_STATE_DISCRIMINATOR,
            &data[..DISCRIMINATOR_LENGTH]
        ));
    }

    let pool_data_bytes = &data[DISCRIMINATOR_LENGTH..];
    let pool_struct: &PoolStateData = from_bytes(pool_data_bytes);

    // --- CRÉATION DE LA SORTIE PROPRE ---
    // On extrait les informations les plus pertinentes pour notre stratégie.
    Ok(DecodedLaunchpadPool {
        address: *address,
        mint_a: pool_struct.base_mint,
        mint_b: pool_struct.quote_mint,
        vault_a: pool_struct.base_vault,
        vault_b: pool_struct.quote_vault,
        total_base_sold: pool_struct.total_base_sell,
        total_quote_raised: pool_struct.real_quote, // 'real_quote' représente les fonds levés
        global_config: pool_struct.global_config,
        virtual_base: pool_struct.virtual_base,
        virtual_quote: pool_struct.virtual_quote,
    })
}