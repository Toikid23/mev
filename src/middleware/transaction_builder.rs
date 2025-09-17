use super::{ExecutionContext, Middleware};
use crate::execution::{fee_manager::FeeManager, transaction_builder, routing};
use crate::state::{
    leader_schedule::LeaderScheduleTracker, slot_metronome::SlotMetronome,
    slot_tracker::SlotTracker, validator_intel::ValidatorIntelService,
};
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
use tracing::{error, info, instrument, warn};
use crate::execution::transaction_builder::ArbitrageInstructionsTemplate;

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
        opportunity_id = context.pool_pair_id,
        // On prépare les champs qu'on remplira plus tard. C'est une bonne pratique.
        target_slot = tracing::field::Empty,
        leader = tracing::field::Empty,
        decision = tracing::field::Empty,
        latency_ms = tracing::field::Empty,
        jito_region = tracing::field::Empty,
    ))]
    async fn process(&self, context: &mut ExecutionContext) -> Result<bool> {
        // --- ON RÉCUPÈRE LA SPAN ACTUELLE POUR POUVOIR LA MODIFIER ---
        let span = tracing::Span::current();

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

        let mut leader_intel_opt = None;
        if let Some(id) = leader_id_opt {
            leader_intel_opt = self.validator_intel.get_validator_intel(&id).await;
        }

        let mut routing_info_opt = leader_intel_opt.as_ref()
            .filter(|intel| intel.is_jito)
            .and_then(routing::get_routing_info);

        let mut latency = routing_info_opt.as_ref().map_or(150, |r| r.estimated_latency_ms) as u128;

        if time_remaining <= BOT_PROCESSING_TIME_MS + latency {
            info!(time_remaining, needed = BOT_PROCESSING_TIME_MS + latency, "Trop tard pour le slot actuel, on vise le suivant.");
            target_slot = current_slot + 1;
            leader_id_opt = self.leader_schedule_tracker.get_leader_for_slot(target_slot);
            leader_intel_opt = None;
            routing_info_opt = None;
            if let Some(id) = leader_id_opt {
                leader_intel_opt = self.validator_intel.get_validator_intel(&id).await;
                routing_info_opt = leader_intel_opt.as_ref()
                    .filter(|intel| intel.is_jito)
                    .and_then(routing::get_routing_info);
                latency = routing_info_opt.as_ref().map_or(150, |r| r.estimated_latency_ms) as u128;
            }
        }

        let leader_identity = match leader_id_opt {
            Some(id) => id,
            None => {
                warn!(slot = target_slot, "Leader introuvable. Abandon.");
                // --- ON ENREGISTRE LA RAISON DE L'ÉCHEC DANS LA SPAN ---
                span.record("decision", "Abandon_LeaderNotFound");
                return Ok(false);
            }
        };

        // --- ENRICHISSEMENT DES LOGS AVEC LES DÉCISIONS PRISES ---
        span.record("target_slot", target_slot);
        span.record("leader", &leader_identity.to_string());
        span.record("latency_ms", latency);

        let is_target_leader_jito = leader_intel_opt.map_or(false, |intel| intel.is_jito);
        let estimated_profit = context.estimated_profit.unwrap();
        let estimated_cus = context.estimated_cus.unwrap();

        let (priority_fee_price_per_cu, jito_tip) = if is_target_leader_jito {
            let tip = self.fee_manager.calculate_jito_tip(estimated_profit, JITO_TIP_PERCENT, estimated_cus);
            let profit_net = estimated_profit.saturating_sub(tip);
            if profit_net < 5000 {
                info!(profit_net, "DÉCISION: Abandon. Profit Jito insuffisant.");
                span.record("decision", "Abandon_JitoProfitTooLow");
                return Ok(false);
            }
            let region = routing_info_opt.as_ref().map(|r| r.region);
            span.record("decision", "PrepareJito");
            span.record("jito_region", &format!("{:?}", region));
            info!(profit_net, jito_tip = tip, "DÉCISION: PRÉPARER BUNDLE JITO.");
            (0, Some(tip))
        } else {
            let fee_per_cu = self.fee_manager.calculate_priority_fee(&[/* ... */], 20).await;
            let total_fee = (estimated_cus * fee_per_cu) / 1_000_000;
            let profit_net = estimated_profit.saturating_sub(total_fee).saturating_sub(5000);
            if profit_net < 5000 {
                info!(profit_net, "DÉCISION: Abandon. Profit normal insuffisant.");
                span.record("decision", "Abandon_NormalProfitTooLow");
                return Ok(false);
            }
            span.record("decision", "PrepareNormal");
            info!(profit_net, priority_fee=total_fee, "DÉCISION: PRÉPARER TRANSACTION NORMALE.");
            (fee_per_cu, None)
        };

        context.is_jito_leader = is_target_leader_jito;
        context.jito_tip = jito_tip;
        context.target_jito_region = routing_info_opt.map(|r| r.region);

        let owned_lookup_table = {
            let managed_lut_address = Pubkey::from_str(MANAGED_LUT_ADDRESS).unwrap();

            // 1. Récupérer les données du compte de la LUT
            let lut_account_data = match context.rpc_client.get_account_data(&managed_lut_address).await {
                Ok(data) => data,
                Err(e) => {
                    // Log critique : sans la LUT, aucune transaction ne peut être construite.
                    error!(error = %e, lut_address = %managed_lut_address, "CRITICAL: Le compte de la LUT est introuvable. Impossible de construire des transactions.");
                    // On arrête le pipeline proprement.
                    return Ok(false);
                }
            };

            // 2. Désérialiser les données en une structure compréhensible
            let lut_ref = match AddressLookupTable::deserialize(&lut_account_data) {
                Ok(lut) => lut,
                Err(e) => {
                    error!(error = %e, lut_address = %managed_lut_address, "CRITICAL: Échec de la désérialisation de la LUT.");
                    return Ok(false);
                }
            };

            // 3. Convertir au format attendu par le constructeur de message
            SdkAddressLookupTableAccount {
                key: managed_lut_address,
                addresses: lut_ref.addresses.to_vec(),
            }
        };

        // --- APPEL COMPLET ET CORRIGÉ ---
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