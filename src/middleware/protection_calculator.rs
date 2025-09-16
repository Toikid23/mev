// DANS : src/middleware/protection_calculator.rs

use super::{ExecutionContext, Middleware};
use crate::execution::protections;
use crate::monitoring::metrics;
use anyhow::{Result, Ok};
use async_trait::async_trait;
use tracing::warn;

pub struct ProtectionCalculator;

#[async_trait]
impl Middleware for ProtectionCalculator {
    fn name(&self) -> &'static str { "ProtectionCalculator" }

    async fn process(&self, context: &mut ExecutionContext) -> Result<bool> {
        // --- CORRECTION ICI ---
        let protections = match protections::calculate_slippage_protections(
            context.opportunity.amount_in,
            context.estimated_profit.unwrap(),
            context.pool_sell_to.as_mut().unwrap().clone(),
            &context.opportunity.token_intermediate_mint,
            context.current_timestamp,
        ) {
            Result::Ok(p) => p, // Utilisez Result::Ok
            Err(e) => {
                warn!(error = %e, "Échec du calcul des protections. Abandon.");
                metrics::TRANSACTION_OUTCOMES.with_label_values(&["ProtectionError", &context.pool_pair_id]).inc();
                return Ok(false);
            }
        };

        context.protections = Some(protections);
        Ok(true)
    }
}