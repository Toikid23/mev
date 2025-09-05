// src/decoders/spl_token_decoders/mint.rs

use anyhow::Result;
use solana_sdk::pubkey::Pubkey;
use spl_token_2022::{
    extension::{BaseStateWithExtensions, StateWithExtensions},
    extension::transfer_fee::TransferFeeConfig,
    state::Mint,
};
use anyhow::anyhow;
use serde::{Serialize, Deserialize};

// --- STRUCTURE DE SORTIE PROPRE ---
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)] // <-- AJOUTER Serialize, Deserialize
pub struct DecodedMint {
    pub address: Pubkey,
    pub supply: u64, // <-- AJOUTER CE CHAMP
    pub decimals: u8,
    pub transfer_fee_basis_points: u16,
    pub max_transfer_fee: u64,
}

pub fn decode_mint(address: &Pubkey, data: &[u8]) -> Result<DecodedMint> {
    let mint_state = StateWithExtensions::<Mint>::unpack(data)?;
    let base_mint = mint_state.base;
    let transfer_fee_config = mint_state.get_extension::<TransferFeeConfig>().ok();

    let (transfer_fee_basis_points, max_transfer_fee) =
        if let Some(config) = transfer_fee_config {
            (config.newer_transfer_fee.transfer_fee_basis_points.into(),
             config.newer_transfer_fee.maximum_fee.into())
        } else {
            (0, 0)
        };

    Ok(DecodedMint {
        address: *address,
        supply: base_mint.supply, // <-- AJOUTER L'INITIALISATION
        decimals: base_mint.decimals,
        transfer_fee_basis_points,
        max_transfer_fee,
    })
}

// ... le reste du fichier (calculate_transfer_fee, etc.) ne change pas
pub fn calculate_transfer_fee(amount: u64, transfer_fee_basis_points: u16, max_fee: u64) -> Result<u64> {
    if transfer_fee_basis_points == 0 { return Ok(0); }
    let fee = (amount as u128)
        .checked_mul(transfer_fee_basis_points as u128)
        .ok_or_else(|| anyhow!("MathOverflow"))?
        .checked_div(10000)
        .ok_or_else(|| anyhow!("MathOverflow"))?;
    Ok(u64::try_from(fee)?.min(max_fee))
}

pub fn calculate_gross_amount_before_transfer_fee(net_amount: u64, transfer_fee_basis_points: u16, max_fee: u64) -> Result<u64> {
    if transfer_fee_basis_points == 0 || net_amount == 0 { return Ok(net_amount); }
    const ONE_IN_BASIS_POINTS: u128 = 10000;
    if transfer_fee_basis_points as u128 >= ONE_IN_BASIS_POINTS { return Ok(net_amount.saturating_add(max_fee)); }
    let numerator = (net_amount as u128).checked_mul(ONE_IN_BASIS_POINTS).ok_or_else(|| anyhow!("MathOverflow"))?;
    let denominator = ONE_IN_BASIS_POINTS.checked_sub(transfer_fee_basis_points as u128).ok_or_else(|| anyhow!("MathOverflow"))?;
    let raw_gross_amount = numerator.div_ceil(denominator);
    let fee = raw_gross_amount.saturating_sub(net_amount as u128);
    if fee >= max_fee as u128 { Ok(net_amount.saturating_add(max_fee)) } else { Ok(raw_gross_amount as u64) }
}