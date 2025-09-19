use anyhow::{Result, anyhow};
use crate::decoders::{Pool, PoolOperations};
use solana_sdk::pubkey::Pubkey;
use tracing::{error, info, warn, debug};

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
    slippage_tolerance_percent: f64,
) -> Result<SwapProtections> {

    let slippage_tolerated_in_lamports = (predicted_profit as f64 * (slippage_tolerance_percent / 100.0)).floor() as u64;
    let expected_total_return = optimal_amount_in.saturating_add(predicted_profit);
    let final_min_amount_out = expected_total_return.saturating_sub(slippage_tolerated_in_lamports);


    // MODIFIÉ : On utilise le timestamp
    let intermediate_min_amount_out = pool_sell_to.get_required_input(
        token_intermediate_mint,
        final_min_amount_out,
        current_timestamp,
    )?;

    debug!(
        predicted_profit,
        slippage_tolerated_lamports = slippage_tolerated_in_lamports,
        expected_total_return,
        final_min_amount_out,
        intermediate_min_amount_out,
        "Calcul du slippage dynamique"
    );


    if intermediate_min_amount_out == 0 {
        return Err(anyhow!("Le calcul de l'input intermédiaire a retourné 0, l'opportunité n'est probablement pas viable."));
    }

    Ok(SwapProtections {
        final_min_amount_out,
        intermediate_min_amount_out,
    })
}