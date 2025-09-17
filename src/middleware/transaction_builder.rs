// DANS : src/middleware/transaction_builder.rs (VERSION FINALE)

use super::{ExecutionContext, Middleware};
use crate::execution::{fee_manager::FeeManager, transaction_builder};
use crate::monitoring::metrics;
use crate::state::{
    leader_schedule::LeaderScheduleTracker, slot_metronome::SlotMetronome,
    slot_tracker::SlotTracker, validator_intel::ValidatorIntelService,
};
use solana_sdk::signature::Signer;
use std::result::Result::Ok;
use anyhow::{Result};
use async_trait::async_trait;
use solana_address_lookup_table_program::state::AddressLookupTable;
use solana_sdk::{message::AddressLookupTableAccount as SdkAddressLookupTableAccount, pubkey::Pubkey};
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::{Arc, RwLock};
use tracing::{error, info, instrument, warn};
use crate::execution::transaction_builder::ArbitrageInstructionsTemplate; // Import du template


const MANAGED_LUT_ADDRESS: &str = "E5h798UBdK8V1L7MvRfi1ppr2vitPUUUUCVqvTyDgKXN";
const BOT_PROCESSING_TIME_MS: u128 = 200;
const JITO_TIP_PERCENT: u64 = 20;

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
    ) -> Self {
        Self {
            slot_tracker,
            slot_metronome,
            leader_schedule_tracker,
            validator_intel,
            fee_manager,
            template_cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

#[async_trait]
impl Middleware for TransactionBuilder {
    fn name(&self) -> &'static str { "TransactionBuilder" }

    #[instrument(name = "transaction_builder_process", skip_all, fields(
        opportunity_id = context.pool_pair_id
    ))]
    async fn process(&self, context: &mut ExecutionContext) -> Result<bool> {
        let template = {
            let reader = self.template_cache.read().unwrap();
            reader.get(&context.pool_pair_id).cloned()
        };

        let template = match template {
            Some(t) => t,
            None => {
                info!(pool_pair = %context.pool_pair_id, "Création d'un nouveau template d'instructions.");
                let new_template = ArbitrageInstructionsTemplate::new(
                    &context.opportunity,
                    &context.graph_snapshot,
                    &context.payer.pubkey()
                )?;
                let mut writer = self.template_cache.write().unwrap();
                writer.insert(context.pool_pair_id.clone(), new_template.clone());
                new_template
            }
        };

        let current_slot = self.slot_tracker.current().clock.slot;
        let time_remaining_in_slot = self.slot_metronome.estimated_time_remaining_in_slot_ms();

        let (target_slot, leader_identity_opt) = if time_remaining_in_slot > BOT_PROCESSING_TIME_MS {
            (current_slot, self.leader_schedule_tracker.get_leader_for_slot(current_slot))
        } else {
            let next_slot = current_slot + 1; // <-- AJOUTER CETTE LIGNE
            (next_slot, self.leader_schedule_tracker.get_leader_for_slot(next_slot))
        };

        let leader_identity = match leader_identity_opt {
            Some(identity) => identity,
            None => {
                warn!(slot = target_slot, "Leader introuvable pour le slot cible. Abandon.");
                metrics::TRANSACTION_OUTCOMES.with_label_values(&["Abandoned_LeaderNotFound", &context.pool_pair_id]).inc();
                return Ok(false);
            }
        };

        let managed_lut_address = Pubkey::from_str(MANAGED_LUT_ADDRESS).unwrap();
        let lut_account_data = match context.rpc_client.get_account_data(&managed_lut_address).await {
            Ok(data) => data,
            Err(e) => { error!(error = %e, "Le compte de la LUT n'a pas été trouvé."); return Ok(false); }
        };
        let lut_ref = AddressLookupTable::deserialize(&lut_account_data)?;
        let owned_lookup_table = SdkAddressLookupTableAccount {
            key: managed_lut_address,
            addresses: lut_ref.addresses.to_vec(),
        };

        let estimated_profit = context.estimated_profit.unwrap();
        let estimated_cus = context.estimated_cus.unwrap();
        let intermediate_amount_out = context.intermediate_amount_out.unwrap();
        let protections = context.protections.as_ref().unwrap();

        let final_tx = if let Some(_validator_info) = self.validator_intel.get_validator_info(&leader_identity).await {
            context.is_jito_leader = true;
            context.span.record("decision", "PrepareJito");
            let jito_tip = self.fee_manager.calculate_jito_tip(estimated_profit, JITO_TIP_PERCENT, estimated_cus);
            let profit_net_final = estimated_profit.saturating_sub(jito_tip);

            const MIN_JITO_PROFIT: u64 = 5000;
            if profit_net_final > MIN_JITO_PROFIT {
                info!(profit_net_final, jito_tip, "DÉCISION : PRÉPARER BUNDLE JITO.");
                context.jito_tip = Some(jito_tip);
                transaction_builder::build_from_template(
                    &template, context.opportunity.amount_in, intermediate_amount_out, protections,
                    estimated_cus, 0, &context.rpc_client, &context.payer, &owned_lookup_table,
                ).await
            } else {
                info!(profit_net_final, threshold = MIN_JITO_PROFIT, "DÉCISION : Abandon. Profit Jito insuffisant.");
                metrics::TRANSACTION_OUTCOMES.with_label_values(&["Abandoned_JitoProfitTooLow", &context.pool_pair_id]).inc();
                return Ok(false);
            }
        } else {
            context.span.record("decision", "PrepareNormal");
            const OVERBID_PERCENT: u8 = 20;
            let accounts_in_tx = vec![context.opportunity.pool_buy_from_key, context.opportunity.pool_sell_to_key];
            let priority_fee_price_per_cu = self.fee_manager.calculate_priority_fee(&accounts_in_tx, OVERBID_PERCENT).await;
            let total_priority_fee = (estimated_cus * priority_fee_price_per_cu) / 1_000_000;
            let profit_net_final = estimated_profit.saturating_sub(5000).saturating_sub(total_priority_fee);

            const MIN_NORMAL_PROFIT: u64 = 5000;
            if profit_net_final > MIN_NORMAL_PROFIT {
                info!(profit_net_final, total_priority_fee, "DÉCISION: PRÉPARER TRANSACTION NORMALE.");
                transaction_builder::build_from_template(
                    &template, context.opportunity.amount_in, intermediate_amount_out, protections,
                    estimated_cus, priority_fee_price_per_cu, &context.rpc_client, &context.payer, &owned_lookup_table,
                ).await
            } else {
                info!(profit_net_final, threshold = MIN_NORMAL_PROFIT, "DÉCISION : Abandon. Profit normal insuffisant.");
                metrics::TRANSACTION_OUTCOMES.with_label_values(&["Abandoned_NormalProfitTooLow", &context.pool_pair_id]).inc();
                return Ok(false);
            }
        };

        match final_tx {
            Ok(tx) => {
                context.final_tx = Some(tx);
                Ok(true)
            }
            Err(e) => {
                error!(error = %e, "Échec de la construction de la transaction. Abandon.");
                metrics::TRANSACTION_OUTCOMES.with_label_values(&["Abandoned_BuildError", &context.pool_pair_id]).inc();
                Ok(false)
            }
        }
    }
}