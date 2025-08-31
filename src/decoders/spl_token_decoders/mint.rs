// src/decoders/spl_token_decoders/mint.rs

use anyhow::Result;
use solana_sdk::pubkey::Pubkey;
use spl_token_2022::{
    extension::{BaseStateWithExtensions, StateWithExtensions},
    extension::transfer_fee::TransferFeeConfig,
    state::Mint,
};
use anyhow::anyhow;

// --- STRUCTURE DE SORTIE PROPRE ---
// Contient les informations que nous extrayons d'un compte de mint.
#[derive(Debug, Clone, PartialEq)]
pub struct DecodedMint {
    pub address: Pubkey,
    pub decimals: u8,
    pub transfer_fee_basis_points: u16, // Les frais en points de base (100 = 1%)
    pub max_transfer_fee: u64,          // Le montant maximum de frais prélevables
}

/// Décode les données brutes d'un compte de mint (SPL Token ou Token-2022)
/// et en extrait les informations essentielles, y compris la taxe de transfert.
pub fn decode_mint(address: &Pubkey, data: &[u8]) -> Result<DecodedMint> {

    // --- Étape 1: Décodage intelligent avec StateWithExtensions ---
    // Cette fonction de la librairie spl-token-2022 est capable de lire
    // à la fois les anciens mints (sans extensions) et les nouveaux.
    let mint_state = StateWithExtensions::<Mint>::unpack(data)?;

    // On récupère la structure de base du mint.
    let base_mint = mint_state.base;

    // --- Étape 2: Recherche de l'extension TransferFee ---
    // On essaie d'extraire la configuration de la taxe de transfert.
    // Si l'extension n'existe pas, la fonction retournera une erreur,
    // que nous transformerons en `None`.
    let transfer_fee_config = mint_state.get_extension::<TransferFeeConfig>().ok();

    // --- Étape 3: On assemble notre structure de sortie propre ---
    let (transfer_fee_basis_points, max_transfer_fee) =
        if let Some(config) = transfer_fee_config {
            // Si la config existe, on extrait les frais.
            // Les valeurs sont stockées dans un type spécial `Pod`, on doit les convertir.
            (config.newer_transfer_fee.transfer_fee_basis_points.into(),
             config.newer_transfer_fee.maximum_fee.into())
        } else {
            // Si la config n'existe pas, les frais sont de 0.
            (0, 0)
        };

    Ok(DecodedMint {
        address: *address,
        decimals: base_mint.decimals,
        transfer_fee_basis_points,
        max_transfer_fee,
    })
}

/// Calcule les frais de transfert pour un montant donné, en utilisant la logique exacte de la librairie SPL.
/// C'est l'équivalent de `calculate_fee` on-chain.
pub fn calculate_transfer_fee(
    amount: u64,
    transfer_fee_basis_points: u16,
    max_transfer_fee: u64
) -> Result<u64> {
    if transfer_fee_basis_points == 0 {
        return Ok(0);
    }
    let fee = (amount as u128)
        .checked_mul(transfer_fee_basis_points as u128)
        .ok_or_else(|| anyhow!("MathOverflow on fee calculation"))?
        .checked_div(10000)
        .ok_or_else(|| anyhow!("MathOverflow on fee calculation"))?;

    Ok(u64::try_from(fee)?.min(max_transfer_fee))
}

/// Calcule le montant brut nécessaire pour qu'après déduction des frais, il reste le montant net.
/// C'est l'équivalent de `calculate_pre_fee_amount` on-chain.
pub fn calculate_gross_amount_before_transfer_fee(
    net_amount: u64,
    transfer_fee_basis_points: u16,
    max_transfer_fee: u64,
) -> Result<u64> {
    if transfer_fee_basis_points == 0 || net_amount == 0 {
        return Ok(net_amount);
    }

    const ONE_IN_BASIS_POINTS: u128 = 10000;

    if transfer_fee_basis_points as u128 >= ONE_IN_BASIS_POINTS {
        return Ok(net_amount.saturating_add(max_transfer_fee));
    }

    let numerator = (net_amount as u128)
        .checked_mul(ONE_IN_BASIS_POINTS)
        .ok_or_else(|| anyhow!("MathOverflow"))?;
    let denominator = ONE_IN_BASIS_POINTS
        .checked_sub(transfer_fee_basis_points as u128)
        .ok_or_else(|| anyhow!("MathOverflow"))?;

    use num_integer::Integer;
    let raw_gross_amount = numerator.div_ceil(denominator);

    let fee = raw_gross_amount.saturating_sub(net_amount as u128);

    if fee >= max_transfer_fee as u128 {
        Ok(net_amount.saturating_add(max_transfer_fee))
    } else {
        Ok(raw_gross_amount as u64)
    }
}