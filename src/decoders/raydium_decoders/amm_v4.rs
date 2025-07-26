// src/decoders/raydium_decoders/amm_v4.rs

use bytemuck::{from_bytes, Pod, Zeroable};
use solana_sdk::pubkey::Pubkey;
use anyhow::{bail, Result};
use crate::decoders::DecodedAmmPool; // Nous réutilisons la structure unifiée

// --- STRUCTURES DE DONNÉES BRUTES (Miroir exact de l'IDL) ---

#[repr(C, packed)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
struct Fees {
    pub min_separate_numerator: u64,
    pub min_separate_denominator: u64,
    pub trade_fee_numerator: u64,
    pub trade_fee_denominator: u64,
    pub pnl_numerator: u64,
    pub pnl_denominator: u64,
    pub swap_fee_numerator: u64,
    pub swap_fee_denominator: u64,
}

#[repr(C, packed)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
struct OutPutData {
    pub need_take_pnl_coin: u64,
    pub need_take_pnl_pc: u64,
    pub total_pnl_pc: u64,
    pub total_pnl_coin: u64,
    pub pool_open_time: u64,
    pub punish_pc_amount: u64,
    pub punish_coin_amount: u64,
    pub orderbook_to_init_time: u64,
    pub swap_coin_in_amount: u128,
    pub swap_pc_out_amount: u128,
    pub swap_take_pc_fee: u64,
    pub swap_pc_in_amount: u128,
    pub swap_coin_out_amount: u128,
    pub swap_take_coin_fee: u64,
}

#[repr(C, packed)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
struct AmmInfoData {
    pub status: u64,
    pub nonce: u64,
    pub order_num: u64,
    pub depth: u64,
    pub coin_decimals: u64,
    pub pc_decimals: u64,
    pub state: u64,
    pub reset_flag: u64,
    pub min_size: u64,
    pub vol_max_cut_ratio: u64,
    pub amount_wave: u64,
    pub coin_lot_size: u64,
    pub pc_lot_size: u64,
    pub min_price_multiplier: u64,
    pub max_price_multiplier: u64,
    pub sys_decimal_value: u64,
    pub fees: Fees,
    pub out_put: OutPutData,
    pub token_coin: Pubkey,
    pub token_pc: Pubkey,
    pub coin_mint: Pubkey,
    pub pc_mint: Pubkey,
    pub lp_mint: Pubkey,
    pub open_orders: Pubkey,
    pub market: Pubkey,
    pub serum_dex: Pubkey,
    pub target_orders: Pubkey,
    pub withdraw_queue: Pubkey,
    pub token_temp_lp: Pubkey,
    pub amm_owner: Pubkey,
    pub lp_amount: u64,
    pub client_order_id: u64,
    pub padding: [u64; 2],
}

/// Tente de décoder les données brutes d'un compte Raydium AMM V4.
pub fn decode_pool(address: &Pubkey, data: &[u8]) -> Result<DecodedAmmPool> {
    // Note: Ces comptes ne sont pas basés sur Anchor, donc pas de discriminator.
    // Les données commencent directement.

    // Étape 1: Vérifier la taille
    if data.len() != std::mem::size_of::<AmmInfoData>() {
        bail!(
            "AMM V4 data length mismatch. Expected {}, got {}.",
            std::mem::size_of::<AmmInfoData>(),
            data.len()
        );
    }

    // Étape 2: "Caster" les données
    let pool_struct: &AmmInfoData = from_bytes(data);

    // Étape 3: Calculer les frais
    let total_fee_percent = if pool_struct.fees.trade_fee_denominator == 0 {
        0.0
    } else {
        pool_struct.fees.trade_fee_numerator as f64 / pool_struct.fees.trade_fee_denominator as f64
    };
    

    // Étape 4: Créer la sortie propre et unifiée
    Ok(DecodedAmmPool {
        address: *address,
        mint_a: pool_struct.coin_mint,
        mint_b: pool_struct.pc_mint,
        vault_a: pool_struct.token_coin,
        vault_b: pool_struct.token_pc,
        total_fee_percent,
    })
}