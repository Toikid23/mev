// DANS : src/middleware/protection_calculator.rs

use super::{ExecutionContext, Middleware};
use crate::execution::protections;
use crate::monitoring::metrics;
use anyhow::{Result};
use async_trait::async_trait;
// --- NOUVEL IMPORT ---
use tracing::{warn, instrument};
use std::result::Result::Ok;

pub struct ProtectionCalculator;

#[async_trait]
impl Middleware for ProtectionCalculator {
    fn name(&self) -> &'static str { "ProtectionCalculator" }

    #[instrument(name = "protection_calculator_process", skip_all, fields(
        opportunity_id = context.pool_pair_id
    ))]
    async fn process(&self, context: &mut ExecutionContext) -> Result<bool> {
        let protections = match protections::calculate_slippage_protections(
            context.opportunity.amount_in,
            context.estimated_profit.unwrap(),
            context.pool_sell_to.as_mut().unwrap().clone(),
            &context.opportunity.token_intermediate_mint,
            context.current_timestamp,
            context.config.slippage_tolerance_percent, // <--- PASSEZ LA VALEUR DE LA CONFIG
        ) {
            Ok(p) => p,
            Err(e) => {
                // --- LOG SPÉCIFIQUE ---
                warn!(error = %e, "Échec du calcul des protections. Opportunité potentiellement non viable. Abandon.");
                metrics::TRANSACTION_OUTCOMES.with_label_values(&["Abandoned_ProtectionError", &context.pool_pair_id]).inc();
                return Ok(false);
            }
        };

        context.protections = Some(protections);
        Ok(true)
    }
}