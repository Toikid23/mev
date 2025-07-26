use anyhow::{anyhow, Result};
use bytemuck::{Pod, Zeroable, from_bytes};
use solana_sdk::pubkey::Pubkey;
use crate::decoders::DecodedAmmPool;

// --- DÉFINITION DE LA STRUCT DE DÉCODAGE (Miroir de la version on-chain) ---
// Cette structure est conçue pour correspondre exactement au layout binaire
// du programme spl-token-swap.

#[repr(C, packed)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
pub struct Fees {
    pub trade_fee_numerator: u64,
    pub trade_fee_denominator: u64,
    pub owner_trade_fee_numerator: u64,
    pub owner_trade_fee_denominator: u64,
    pub owner_withdraw_fee_numerator: u64,
    pub owner_withdraw_fee_denominator: u64,
    pub host_fee_numerator: u64,
    pub host_fee_denominator: u64,
}

#[repr(C, packed)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
pub struct SwapCurve {
    pub curve_type: u8,
    // La suite de la courbe dépend du curve_type, c'est complexe.
    // Pour un décodage simple, on peut la représenter comme un tableau de bytes.
    pub curve_parameters: [u8; 32],
}

#[repr(C, packed)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
pub struct OrcaTokenSwapV2Pool {
    pub is_initialized: u8,     // bool est 1 byte
    pub bump_seed: u8,
    pub token_program_id: Pubkey,
    pub token_a_vault: Pubkey,  // Nommé `token_a` dans le code source
    pub token_b_vault: Pubkey,  // Nommé `token_b` dans le code source
    pub pool_mint: Pubkey,
    pub token_a_mint: Pubkey,
    pub token_b_mint: Pubkey,
    pub pool_fee_account: Pubkey,
    pub fees: Fees,
    pub swap_curve: SwapCurve,
}



/// Tente de décoder les données brutes et les transforme en une structure unifiée.
pub fn decode_pool(address: &Pubkey, data: &[u8]) -> Result<DecodedAmmPool> {
    const EXPECTED_DATA_LEN: usize = 324;

    if data.len() != EXPECTED_DATA_LEN {
        return Err(anyhow!(
            "Invalid data length for Orca V2 Pool. Expected {} bytes, got {}",
            EXPECTED_DATA_LEN,
            data.len()
        ));
    }

    let pool_data_bytes = &data[1..];
    let pool_struct: &OrcaTokenSwapV2Pool = from_bytes(pool_data_bytes);

    // --- CALCUL DES FRAIS ---
    // Les frais sont stockés sous forme de fraction (numérateur / dénominateur).
    // On les convertit en pourcentage.
    let total_fee_percent = if pool_struct.fees.trade_fee_denominator == 0 {
        0.0
    } else {
        pool_struct.fees.trade_fee_numerator as f64 / pool_struct.fees.trade_fee_denominator as f64
    };
    

    Ok(DecodedAmmPool {
        address: *address,
        mint_a: pool_struct.token_a_mint,
        mint_b: pool_struct.token_b_mint,
        vault_a: pool_struct.token_a_vault,
        vault_b: pool_struct.token_b_vault,
        total_fee_percent,
    })
}