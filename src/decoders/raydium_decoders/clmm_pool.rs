// src/decoders/raydium_decoders/clmm_pool.rs

use bytemuck::{from_bytes, Pod, Zeroable};
use solana_sdk::pubkey::Pubkey;
use anyhow::{bail, Result};

// --- STRUCTURE DE SORTIE PROPRE ---
#[derive(Debug, Clone)]
pub struct DecodedClmmPool {
    pub address: Pubkey,
    pub amm_config: Pubkey,
    pub mint_a: Pubkey,
    pub mint_b: Pubkey,
    pub vault_a: Pubkey,
    pub vault_b: Pubkey,
    pub sqrt_price_x64: u128,
    pub tick_current: i32,
}

// Discriminator pour les comptes PoolState du programme CLMM
const CLMM_POOL_STATE_DISCRIMINATOR: [u8; 8] = [247, 237, 227, 245, 215, 195, 222, 70];


// --- STRUCTURES BRUTES (Miroir de l'IDL) ---

#[repr(C, packed)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
struct RewardInfo {
    pub reward_state: u8,
    pub open_time: u64,
    pub end_time: u64,
    pub last_update_time: u64,
    pub emissions_per_second_x64: u128,
    pub reward_total_emissioned: u64,
    pub reward_claimed: u64,
    pub token_mint: Pubkey,
    pub token_vault: Pubkey,
    pub authority: Pubkey,
    pub reward_growth_global_x64: u128,
}

#[repr(C, packed)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
struct ClmmPoolStateData {
    pub bump: [u8; 1],
    pub amm_config: Pubkey,
    pub owner: Pubkey,
    pub token_mint_0: Pubkey,
    pub token_mint_1: Pubkey,
    pub token_vault_0: Pubkey,
    pub token_vault_1: Pubkey,
    pub observation_key: Pubkey,
    pub mint_decimals_0: u8,
    pub mint_decimals_1: u8,
    pub tick_spacing: u16,
    pub liquidity: u128,
    pub sqrt_price_x64: u128,
    pub tick_current: i32,
    pub padding3: u16,
    pub padding4: u16,
    pub fee_growth_global_0_x64: u128,
    pub fee_growth_global_1_x64: u128,
    pub protocol_fees_token_0: u64,
    pub protocol_fees_token_1: u64,
    pub swap_in_amount_token_0: u128,
    pub swap_out_amount_token_1: u128,
    pub swap_in_amount_token_1: u128,
    pub swap_out_amount_token_0: u128,
    pub status: u8,
    pub _padding4: [u8; 7],
    pub reward_infos: [RewardInfo; 3],
    pub tick_array_bitmap: [u64; 16],
    pub total_fees_token_0: u64,
    pub total_fees_claimed_token_0: u64,
    pub total_fees_token_1: u64,
    pub total_fees_claimed_token_1: u64,
    pub fund_fees_token_0: u64,
    pub fund_fees_token_1: u64,
    pub open_time: u64,
    pub recent_epoch: u64,
    pub padding1: [u64; 24],
    pub padding2: [u64; 32],
}

/// Décode le compte PoolState d'un pool Raydium CLMM.
pub fn decode_pool(address: &Pubkey, data: &[u8]) -> Result<DecodedClmmPool> {
    if data.get(..8) != Some(&CLMM_POOL_STATE_DISCRIMINATOR) {
        bail!("Invalid discriminator. Not a Raydium CLMM PoolState account.");
    }
    let data_slice = &data[8..];
    if data_slice.len() != std::mem::size_of::<ClmmPoolStateData>() {
        bail!(
            "CLMM PoolState data length mismatch. Expected {}, got {}.",
            std::mem::size_of::<ClmmPoolStateData>(),
            data_slice.len()
        );
    }

    let pool_struct: &ClmmPoolStateData = from_bytes(data_slice);

    Ok(DecodedClmmPool {
        address: *address,
        amm_config: pool_struct.amm_config,
        mint_a: pool_struct.token_mint_0,
        mint_b: pool_struct.token_mint_1,
        vault_a: pool_struct.token_vault_0,
        vault_b: pool_struct.token_vault_1,
        sqrt_price_x64: pool_struct.sqrt_price_x64,
        tick_current: pool_struct.tick_current,
    })
}