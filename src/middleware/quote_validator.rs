use super::{ExecutionContext, Middleware};
use crate::decoders::PoolOperations;
use crate::execution::cu_manager;
use crate::monitoring::metrics;
use anyhow::{Result};
use async_trait::async_trait;
use tracing::{info, warn, instrument, error};
use std::result::Result::Ok;
use crate::state::balance_tracker::{BalanceHistory, HISTORY_WINDOW_SECS};
use std::time::{SystemTime, UNIX_EPOCH};
// --- AJOUTER L'IMPORT ---
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
        // --- VÉRIFICATION KILL-SWITCH À FENÊTRE GLISSANTE ---
        match BalanceHistory::load().await {
            Ok(history) => {
                let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
                let recent_entries: Vec<_> = history.entries.iter()
                    .filter(|e| now.saturating_sub(e.timestamp) < HISTORY_WINDOW_SECS)
                    .collect();

                if !recent_entries.is_empty() {
                    let high_water_mark = recent_entries.iter().map(|e| e.lamports).max().unwrap_or(0);
                    let current_balance = recent_entries.last().map_or(0, |e| e.lamports);

                    if current_balance > 0 && high_water_mark > 0 {
                        let drawdown = (high_water_mark as i128) - (current_balance as i128);
                        let max_loss = context.config.max_cumulative_loss as i128;

                        if drawdown > max_loss {
                            error!(
                                high_water_mark,
                                current_balance,
                                drawdown,
                                max_loss,
                                "KILL-SWITCH ACTIVÉ : Perte maximale sur 24h dépassée. Arrêt du trading."
                            );
                            std::process::exit(1);
                        }
                    }
                }
            }
            Err(e) => {
                warn!(error = %e, "Impossible de charger l'historique des soldes. Le trading continue, mais le kill-switch est potentiellement inopérant.");
            }
        }
        // --- FIN DE LA VÉRIFICATION ---
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

        // --- ON UTILISE LA VALEUR DE LA CONFIGURATION ---
        let min_profit_threshold = context.config.min_profit_threshold;

        if profit_brut_estime < min_profit_threshold {
            info!(
                revalidated_profit = profit_brut_estime,
                threshold = min_profit_threshold, // On log le seuil utilisé
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
        context.intermediate_amount_out = Some(quote_buy_details.amount_out);
        context.estimated_cus = Some(estimated_cus);

        Ok(true)
    }
}