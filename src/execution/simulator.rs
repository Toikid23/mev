// DANS: src/execution/simulator.rs

use anyhow::{anyhow, Result};
use solana_client::{
    rpc_config::RpcSimulateTransactionConfig,
    rpc_response::RpcPrioritizationFee,
};
use solana_sdk::{pubkey::Pubkey, transaction::VersionedTransaction};
use solana_transaction_status::UiTransactionEncoding;
use std::sync::Arc;
use crate::rpc::ResilientRpcClient;

// NOUVEAU : On définit une structure pour un retour propre
pub struct SimulationResult {
    pub profit_brut_reel: u64,
    pub compute_units: u64,
    pub priority_fees: Vec<RpcPrioritizationFee>,
}

/// Extrait le profit réalisé à partir des logs d'une simulation réussie.
fn parse_profit_from_logs(logs: &[String]) -> Option<u64> {
    let prefix = "Program log: SUCCÈS ! Profit net réalisé: ";
    for log in logs {
        if let Some(profit_str) = log.strip_prefix(prefix) {
            return profit_str.parse::<u64>().ok();
        }
    }
    None
}

/// Lance les simulations et la récupération des frais en parallèle.
pub async fn run_simulations(
    rpc_client: Arc<ResilientRpcClient>,
    transaction: &VersionedTransaction,
    accounts_for_fees: Vec<Pubkey>,
) -> Result<SimulationResult> {
    println!("\n--- [Phase 2] Lancement des simulations et de la récupération des frais ---");
    let sim_config = RpcSimulateTransactionConfig {
        sig_verify: false,
        replace_recent_blockhash: true,
        commitment: Some(rpc_client.commitment()),
        encoding: Some(UiTransactionEncoding::Base64),
        ..Default::default()
    };

    // Votre tokio::join! original, maintenant dans sa propre fonction.
    let (sim_result, jito_sim_result, priority_fees_result) = tokio::join!(
        rpc_client.simulate_transaction_with_config(transaction, sim_config.clone()),
        rpc_client.simulate_transaction_with_config(transaction, sim_config),
        rpc_client.get_recent_prioritization_fees(&accounts_for_fees)
    );

    println!("\n--- [Phase 3] Analyse des résultats ---");
    let sim_response = match jito_sim_result.or(sim_result) {
        Ok(response) => response,
        Err(e) => return Err(anyhow!("Les deux simulations ont échoué. Erreur RPC : {}", e)),
    };

    let sim_value = sim_response.value;
    if let Some(err) = sim_value.err {
        if let Some(logs) = sim_value.logs {
            logs.iter().for_each(|log| println!("       {}", log));
        }
        return Err(anyhow!("Simulation ÉCHOUÉE : {:?}", err));
    }
    println!("  -> Simulation RÉUSSIE !");

    let compute_units = sim_value.units_consumed.unwrap_or(0);
    let logs = sim_value.logs.unwrap_or_default();

    let profit_brut_reel = parse_profit_from_logs(&logs)
        .ok_or_else(|| anyhow!("ERREUR : Impossible d'extraire le profit des logs de simulation."))?;

    println!("     -> Profit Brut Réel (simulé) : {} lamports", profit_brut_reel);
    println!("     -> Compute Units Consommés   : {}", compute_units);

    let priority_fees = match priority_fees_result {
        Ok(fees) => fees,
        Err(e) => {
            println!("  -> AVERTISSEMENT : Impossible de récupérer les frais de priorité : {}", e);
            vec![] // Retourne un vecteur vide en cas d'erreur
        }
    };

    Ok(SimulationResult {
        profit_brut_reel,
        compute_units,
        priority_fees,
    })
}