use anyhow::{Result, anyhow};
use crate::decoders::{Pool, PoolOperations};
use solana_sdk::pubkey::Pubkey;

pub struct SwapProtections {
    pub final_min_amount_out: u64,
    pub intermediate_min_amount_out: u64,
}

// MODIFIÉ : La signature accepte `current_timestamp`
pub fn calculate_slippage_protections(
    optimal_amount_in: u64,
    predicted_profit: u64,
    mut pool_sell_to: Pool,
    token_intermediate_mint: &Pubkey,
    current_timestamp: i64,
) -> Result<SwapProtections> {
    println!("\n--- [Protection] Calcul du Slippage Dynamique ---");

    const SLIPPAGE_TOLERANCE_PERCENT: f64 = 0.25;
    let slippage_tolerated_in_lamports = (predicted_profit as f64 * SLIPPAGE_TOLERANCE_PERCENT).floor() as u64;
    let expected_total_return = optimal_amount_in.saturating_add(predicted_profit);
    let final_min_amount_out = expected_total_return.saturating_sub(slippage_tolerated_in_lamports);

    println!("     -> Profit Prédit      : {} lamports", predicted_profit);
    println!("     -> Slippage Toléré (25%): {} lamports", slippage_tolerated_in_lamports);
    println!("     -> Retour Total Attendu : {} lamports", expected_total_return);
    println!("     -> Minimum Final Accepté: {} lamports", final_min_amount_out);

    // MODIFIÉ : On utilise le timestamp
    let intermediate_min_amount_out = pool_sell_to.get_required_input(
        token_intermediate_mint,
        final_min_amount_out,
        current_timestamp,
    )?;

    println!("     -> Input Intermédiaire Requis (pour garantir le min final): {}", intermediate_min_amount_out);

    if intermediate_min_amount_out == 0 {
        return Err(anyhow!("Le calcul de l'input intermédiaire a retourné 0, l'opportunité n'est probablement pas viable."));
    }

    Ok(SwapProtections {
        final_min_amount_out,
        intermediate_min_amount_out,
    })
}