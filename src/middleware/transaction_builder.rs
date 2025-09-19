// DANS : src/middleware/transaction_builder.rs

use super::{ExecutionContext, Middleware};
use crate::execution::{fee_manager::FeeManager, transaction_builder, routing};
use crate::state::{
    leader_schedule::LeaderScheduleTracker, slot_metronome::SlotMetronome,
    slot_tracker::SlotTracker, validator_intel::ValidatorIntelService,
};
use crate::execution::routing::RuntimeConfig;
use std::fs;
use solana_sdk::signature::Signer;
use anyhow::Result;
use async_trait::async_trait;
use solana_address_lookup_table_program::state::AddressLookupTable;
use solana_sdk::{
    message::AddressLookupTableAccount as SdkAddressLookupTableAccount,
    pubkey::Pubkey,
};

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::{Arc, RwLock};
use tracing::{error, info, instrument, warn, debug};
use crate::execution::transaction_builder::ArbitrageInstructionsTemplate;

// --- CORRECTION : AJOUT DE LA CONSTANTE ICI ---
const MANAGED_LUT_ADDRESS: &str = "E5h798UBdK8V1L7MvRfi1ppr2vitPUUUUCVqvTyDgKXN";
// --- FIN DE LA CORRECTION ---

pub struct TransactionBuilder {
    pub slot_tracker: Arc<SlotTracker>,
    pub slot_metronome: Arc<SlotMetronome>,
    pub leader_schedule_tracker: Arc<LeaderScheduleTracker>,
    pub validator_intel: Arc<ValidatorIntelService>,
    pub fee_manager: FeeManager,
    template_cache: Arc<RwLock<HashMap<String, ArbitrageInstructionsTemplate>>>,
}

impl TransactionBuilder {
    pub fn new(
        slot_tracker: Arc<SlotTracker>,
        slot_metronome: Arc<SlotMetronome>,
        leader_schedule_tracker: Arc<LeaderScheduleTracker>,
        validator_intel: Arc<ValidatorIntelService>,
        fee_manager: FeeManager,
        template_cache: Arc<RwLock<HashMap<String, ArbitrageInstructionsTemplate>>>,
    ) -> Self {
        Self {
            slot_tracker,
            slot_metronome,
            leader_schedule_tracker,
            validator_intel,
            fee_manager,
            template_cache,
        }
    }
}

#[async_trait]
impl Middleware for TransactionBuilder {
    fn name(&self) -> &'static str { "TransactionBuilder" }

    #[instrument(name = "transaction_builder_process", skip_all, fields(
        opportunity_id = context.pool_pair_id,
        target_slot = tracing::field::Empty,
        leader = tracing::field::Empty,
        decision = tracing::field::Empty,
        latency_ms = tracing::field::Empty,
        jito_region = tracing::field::Empty,
    ))]
    async fn process(&self, context: &mut ExecutionContext) -> Result<bool> {
        let span = tracing::Span::current();

        debug!(
            estimated_profit = context.estimated_profit,
            estimated_cus = context.estimated_cus,
            "Entrée dans le TransactionBuilder"
        );

        let runtime_config: RuntimeConfig = fs::read_to_string("runtime_config.json")
            .ok()
            .and_then(|data| serde_json::from_str(&data).ok())
            .unwrap_or_default();

        let template = {
            let reader = self.template_cache.read().unwrap();
            if let Some(t) = reader.get(&context.pool_pair_id) {
                t.clone()
            } else {
                drop(reader);
                info!(pool_pair = %context.pool_pair_id, "Création d'un nouveau template d'instructions.");
                let new_template = ArbitrageInstructionsTemplate::new(
                    &context.opportunity, &context.graph_snapshot, &context.payer.pubkey()
                )?;
                let mut writer = self.template_cache.write().unwrap();
                writer.insert(context.pool_pair_id.clone(), new_template.clone());
                new_template
            }
        };

        let time_remaining = self.slot_metronome.estimated_time_remaining_in_slot_ms();
        let current_slot = self.slot_tracker.current().clock.slot;
        let mut target_slot = current_slot;

        let mut leader_id_opt = self.leader_schedule_tracker.get_leader_for_slot(current_slot);
        let mut leader_intel_opt = if let Some(id) = leader_id_opt { self.validator_intel.get_validator_intel(&id).await } else { None };

        let mut routing_info_opt = leader_intel_opt.as_ref()
            .filter(|intel| intel.is_jito)
            .and_then(|intel| routing::get_routing_info(intel, &runtime_config));

        let mut latency = routing_info_opt.as_ref().map_or(150, |r| r.estimated_latency_ms) as u128;
        let time_needed_ms = latency + context.config.bot_processing_time_ms as u128 + context.config.transaction_send_safety_margin_ms as u128;

        if time_remaining <= time_needed_ms {
            info!(time_remaining, needed = time_needed_ms, "Trop tard pour le slot actuel, on vise le suivant.");
            target_slot = current_slot + 1;
            leader_id_opt = self.leader_schedule_tracker.get_leader_for_slot(target_slot);
            leader_intel_opt = if let Some(id) = leader_id_opt { self.validator_intel.get_validator_intel(&id).await } else { None };
            routing_info_opt = leader_intel_opt.as_ref()
                .filter(|intel| intel.is_jito)
                .and_then(|intel| routing::get_routing_info(intel, &runtime_config));
            latency = routing_info_opt.as_ref().map_or(150, |r| r.estimated_latency_ms) as u128;
        }

        let leader_identity = match leader_id_opt {
            Some(id) => id,
            None => {
                warn!(slot = target_slot, "Leader introuvable. Abandon.");
                span.record("decision", "Abandon_LeaderNotFound");
                return Ok(false);
            }
        };

        span.record("target_slot", target_slot);
        span.record("leader", &leader_identity.to_string());

        let is_target_leader_jito = leader_intel_opt.map_or(false, |intel| intel.is_jito);
        let estimated_profit = context.estimated_profit.unwrap();
        let estimated_cus = context.estimated_cus.unwrap();

        let (priority_fee_price_per_cu, jito_tip) = if is_target_leader_jito {
            let tip = self.fee_manager.calculate_jito_tip(estimated_profit, context.config.jito_tip_percent, estimated_cus);
            let profit_net = estimated_profit.saturating_sub(tip);
            if profit_net < 5000 {
                info!(profit_net, "DÉCISION: Abandon. Profit Jito insuffisant.");
                span.record("decision", "Abandon_JitoProfitTooLow");
                return Ok(false);
            }
            let region_name = routing_info_opt.as_ref().map_or("Unknown", |r| match r.region {
                routing::JitoRegion::Amsterdam => "Amsterdam",
                routing::JitoRegion::Dublin => "Dublin",
                routing::JitoRegion::Frankfurt => "Frankfurt",
                routing::JitoRegion::London => "London",
                routing::JitoRegion::NewYork => "NewYork",
                routing::JitoRegion::SaltLakeCity => "SaltLakeCity",
                routing::JitoRegion::Singapore => "Singapore",
                routing::JitoRegion::Tokyo => "Tokyo",
            });
            span.record("jito_region", region_name);
            span.record("latency_ms", latency);
            info!(
                decision = "PrepareJitoBundle",
                profit_net = estimated_profit.saturating_sub(tip),
                jito_tip = tip,
                target_region = region_name,
                latency_ms = latency,
                "Leader Jito détecté, préparation du bundle"
            );
            (0, Some(tip))
        } else {
            let time_remaining_ms = if target_slot == current_slot {
                self.slot_metronome.estimated_time_remaining_in_slot_ms()
            } else {
                self.slot_metronome.average_slot_duration_ms()
            };

            let accounts_in_opp = [
                context.opportunity.pool_buy_from_key,
                context.opportunity.pool_sell_to_key,
            ];

            let fee_per_cu = self.fee_manager.calculate_priority_fee(
                &accounts_in_opp,
                time_remaining_ms
            ).await;

            let total_fee = (estimated_cus * fee_per_cu) / 1_000_000;
            let profit_net = estimated_profit.saturating_sub(total_fee).saturating_sub(5000);

            if profit_net < 5000 {
                info!(profit_net, "DÉCISION: Abandon. Profit normal insuffisant après frais dynamiques.");
                span.record("decision", "Abandon_NormalProfitTooLow");
                return Ok(false);
            }

            span.record("decision", "PrepareNormal");
            info!(
                decision = "PrepareNormalTx",
                profit_net,
                priority_fee_per_cu = fee_per_cu,
                total_priority_fee = total_fee,
                time_remaining_in_slot_ms = time_remaining_ms,
                "Leader non-Jito ou routage impossible, préparation d'une transaction normale"
            );
            (fee_per_cu, None)
        };

        context.is_jito_leader = is_target_leader_jito;
        context.jito_tip = jito_tip;
        context.target_jito_region = routing_info_opt.map(|r| r.region);

        let owned_lookup_table = {
            let managed_lut_address = Pubkey::from_str(MANAGED_LUT_ADDRESS).unwrap();
            let lut_account_data = match context.rpc_client.get_account_data(&managed_lut_address).await {
                Ok(data) => data,
                Err(e) => {
                    error!(error = %e, lut_address = %managed_lut_address, "CRITICAL: Le compte de la LUT est introuvable. Impossible de construire des transactions.");
                    return Ok(false);
                }
            };
            let lut_ref = match AddressLookupTable::deserialize(&lut_account_data) {
                Ok(lut) => lut,
                Err(e) => {
                    error!(error = %e, lut_address = %managed_lut_address, "CRITICAL: Échec de la désérialisation de la LUT.");
                    return Ok(false);
                }
            };
            SdkAddressLookupTableAccount {
                key: managed_lut_address,
                addresses: lut_ref.addresses.to_vec(),
            }
        };

        let final_tx_result = transaction_builder::build_from_template(
            &template,
            context.opportunity.amount_in,
            context.intermediate_amount_out.unwrap(),
            context.protections.as_ref().unwrap(),
            estimated_cus,
            priority_fee_price_per_cu,
            &context.rpc_client,
            &context.payer,
            &owned_lookup_table,
        ).await;

        match final_tx_result {
            Ok(tx) => {
                context.final_tx = Some(tx);
                Ok(true)
            }
            Err(e) => {
                error!(error = %e, "Échec construction TX. Abandon.");
                Ok(false)
            }
        }
    }
}