// DANS : src/middleware/quote_validator.rs

use super::{ExecutionContext, Middleware};
use crate::decoders::PoolOperations;
use crate::execution::cu_manager;
use crate::monitoring::metrics;
use anyhow::{Result};
use async_trait::async_trait;
// --- NOUVEL IMPORT ---
use tracing::{info, warn, instrument};
use std::result::Result::Ok;

pub struct QuoteValidator;

#[async_trait]
impl Middleware for QuoteValidator {
    fn name(&self) -> &'static str {
        "QuoteValidator"
    }

    // --- ON AJOUTE L'ATTRIBUT `instrument` ---
    // Cela va automatiquement créer une "span" pour cette fonction dans vos logs JSON,
    // avec les champs de l'opportunité. `skip_all` évite de logger les gros objets.
    #[instrument(name = "quote_validator_process", skip_all, fields(
        opportunity_id = context.pool_pair_id,
        initial_profit_est = context.opportunity.profit_in_lamports
    ))]
    async fn process(&self, context: &mut ExecutionContext) -> Result<bool> {
        let pool_buy_from = match context.graph_snapshot.pools.get(&context.opportunity.pool_buy_from_key) {
            Some(p) => p.clone(),
            None => {
                // Log plus spécifique
                warn!("Pool (buy) non trouvé dans le graphe. Abandon.");
                metrics::TRANSACTION_OUTCOMES.with_label_values(&["InternalError_PoolNotFound", &context.pool_pair_id]).inc();
                return Ok(false);
            }
        };

        let pool_sell_to = match context.graph_snapshot.pools.get(&context.opportunity.pool_sell_to_key) {
            Some(p) => p.clone(),
            None => {
                warn!("Pool (sell) non trouvé dans le graphe. Abandon.");
                metrics::TRANSACTION_OUTCOMES.with_label_values(&["InternalError_PoolNotFound", &context.pool_pair_id]).inc();
                return Ok(false);
            }
        };

        let quote_buy_details = match pool_buy_from.get_quote_with_details(&context.opportunity.token_in_mint, context.opportunity.amount_in, context.current_timestamp) {
            Ok(q) => q,
            Err(e) => {
                warn!(error = %e, "Échec du calcul de quote (buy). Abandon.");
                metrics::TRANSACTION_OUTCOMES.with_label_values(&["Abandoned_QuoteError", &context.pool_pair_id]).inc();
                return Ok(false);
            }
        };

        let quote_sell_details = match pool_sell_to.get_quote_with_details(&context.opportunity.token_intermediate_mint, quote_buy_details.amount_out, context.current_timestamp) {
            Ok(q) => q,
            Err(e) => {
                warn!(error = %e, "Échec du calcul de quote (sell). Abandon.");
                metrics::TRANSACTION_OUTCOMES.with_label_values(&["Abandoned_QuoteError", &context.pool_pair_id]).inc();
                return Ok(false);
            }
        };

        let profit_brut_estime = quote_sell_details.amount_out.saturating_sub(context.opportunity.amount_in);

        // --- LOG SPÉCIFIQUE POUR LE SEUIL DE PROFIT ---
        const MIN_PROFIT_THRESHOLD: u64 = 50000;
        if profit_brut_estime < MIN_PROFIT_THRESHOLD {
            info!(
                revalidated_profit = profit_brut_estime,
                threshold = MIN_PROFIT_THRESHOLD,
                "Abandon. Profit brut ré-évalué insuffisant."
            );
            metrics::TRANSACTION_OUTCOMES.with_label_values(&["Abandoned_ProfitTooLow", &context.pool_pair_id]).inc();
            return Ok(false);
        }

        let estimated_cus = cu_manager::estimate_arbitrage_cost(&pool_buy_from, quote_buy_details.ticks_crossed, &pool_sell_to, quote_sell_details.ticks_crossed);

        info!(
            revalidated_profit = profit_brut_estime,
            estimated_cus,
            "Validation de l'opportunité réussie."
        );

        context.pool_buy_from = Some(pool_buy_from);
        context.pool_sell_to = Some(pool_sell_to);
        context.estimated_profit = Some(profit_brut_estime);
        context.estimated_cus = Some(estimated_cus);

        Ok(true)
    }
}