// DANS: src/execution/protections.rs

use anyhow::{Result, anyhow};
use crate::decoders::{Pool, PoolOperations};
use solana_sdk::pubkey::Pubkey;

// NOUVEAU: Une structure pour contenir les paramètres de protection calculés.
pub struct SwapProtections {
    // Le montant de SOL minimum que nous devons recevoir à la FIN du 2ème swap.
    pub final_min_amount_out: u64,

    // La quantité de jeton intermédiaire que nous devons envoyer au 2ème swap
    // pour garantir notre `final_min_amount_out`.
    // Ce sera le `min_amount_out` du 1er swap.
    pub intermediate_min_amount_out: u64,
}

/// Calcule les protections de slippage dynamiques pour une opportunité d'arbitrage.
pub fn calculate_slippage_protections(
    // L'opportunité trouvée par l'optimiseur.
    optimal_amount_in: u64,
    predicted_profit: u64,
    // Une copie MUTABLE du pool où nous vendons, pour appeler get_required_input.
    mut pool_sell_to: Pool,
    // Le mint du jeton intermédiaire (ex: USDC).
    token_intermediate_mint: &Pubkey,
) -> Result<SwapProtections> {
    println!("\n--- [Protection] Calcul du Slippage Dynamique ---");

    // --- Stratégie de Slippage ---
    // Nous sommes prêts à sacrifier jusqu'à 25% de notre profit théorique au slippage.
    const SLIPPAGE_TOLERANCE_PERCENT: f64 = 0.25;

    let slippage_tolerated_in_lamports = (predicted_profit as f64 * SLIPPAGE_TOLERANCE_PERCENT).floor() as u64;

    // Le montant total que nous attendons à la fin est notre investissement + le profit.
    let expected_total_return = optimal_amount_in.saturating_add(predicted_profit);

    // Notre protection finale : nous n'accepterons pas moins que ce montant.
    let final_min_amount_out = expected_total_return.saturating_sub(slippage_tolerated_in_lamports);

    println!("     -> Profit Prédit      : {} lamports", predicted_profit);
    println!("     -> Slippage Toléré (25%): {} lamports", slippage_tolerated_in_lamports);
    println!("     -> Retour Total Attendu : {} lamports", expected_total_return);
    println!("     -> Minimum Final Accepté: {} lamports", final_min_amount_out);


    // --- Calcul de la Protection Intermédiaire ---
    // C'est l'étape la plus importante. On se demande :
    // "Pour garantir que je reçoive au moins `final_min_amount_out` de SOL,
    // quelle est la quantité de jeton intermédiaire que je dois absolument envoyer au 2ème pool ?"
    let intermediate_min_amount_out = pool_sell_to.get_required_input(
        token_intermediate_mint,
        final_min_amount_out,
        0, // timestamp n'est pas critique ici, car on veut une garantie mathématique
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