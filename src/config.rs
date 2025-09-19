// DANS : src/config.rs

use serde::Deserialize;
use anyhow::Result;
use solana_sdk::signature::Keypair;
use serde::de::{self, Deserializer, Visitor};
use std::fmt;

#[derive(Deserialize, Debug, Clone)]
pub struct Config {
    // --- Paramètres Existants ---
    pub solana_rpc_url: String,
    pub payer_private_key: String,
    pub birdeye_api_key: String,
    #[serde(default = "default_min_profit_threshold")]
    pub min_profit_threshold: u64,
    #[serde(default = "default_max_cumulative_loss")]
    pub max_cumulative_loss: u64,
    #[serde(default = "default_min_sol_balance")]
    pub min_sol_balance: u64,
    #[serde(default = "default_unwrap_amount")]
    pub unwrap_amount: u64,
    #[serde(default = "default_max_trade_size_sol")]
    pub max_trade_size_sol: f64,
    #[serde(default = "default_safety_margin_ms")]
    pub transaction_send_safety_margin_ms: u64,
    #[serde(default = "default_true")]
    pub dry_run: bool,

    // --- NOUVEAUX PARAMÈTRES CENTRALISÉS ---

    // Workers & Pipeline
    #[serde(default = "default_analysis_worker_count")]
    pub analysis_worker_count: usize,
    #[serde(default = "default_bot_processing_time_ms")]
    pub bot_processing_time_ms: u64, // Changez u128 en u64

    // Circuit Breaker (Disjoncteur)
    #[serde(default = "default_circuit_breaker_failure_threshold")]
    pub circuit_breaker_failure_threshold: usize,
    #[serde(default = "default_circuit_breaker_cooldown_secs")]
    pub circuit_breaker_cooldown_secs: u64,
    #[serde(default = "default_circuit_breaker_blacklist_threshold")]
    pub circuit_breaker_blacklist_threshold: usize,

    // Protections & Frais
    #[serde(default = "default_slippage_tolerance_percent")]
    pub slippage_tolerance_percent: f64,
    #[serde(default = "default_jito_tip_percent")]
    pub jito_tip_percent: u64,

    // Market Scanner
    #[serde(default = "default_hot_transaction_threshold")]
    pub hot_transaction_threshold: usize,
    #[serde(default = "default_activity_window_secs")]
    pub activity_window_secs: u64,

    // Liste des adresses de portefeuille à surveiller pour le copy-trading.
    // Dans .env, séparez-les par des virgules : COPY_TRADE_WALLETS="addr1,addr2,addr3"
    #[serde(default, deserialize_with = "deserialize_string_list")]
    pub copy_trade_wallets: Vec<String>,
}

// --- Fonctions de valeur par défaut ---

fn default_true() -> bool { true }
fn default_safety_margin_ms() -> u64 { 50 }
fn default_max_trade_size_sol() -> f64 { 10.0 }
fn default_min_profit_threshold() -> u64 { 50000 }
fn default_max_cumulative_loss() -> u64 { 100_000_000 }
fn default_min_sol_balance() -> u64 { 50_000_000 }
fn default_unwrap_amount() -> u64 { 50_000_000 }

// Nouvelles fonctions de valeur par défaut
fn default_analysis_worker_count() -> usize { 4 }
fn default_bot_processing_time_ms() -> u64 { 50 }
fn default_circuit_breaker_failure_threshold() -> usize { 5 }
fn default_circuit_breaker_cooldown_secs() -> u64 { 3600 }
fn default_circuit_breaker_blacklist_threshold() -> usize { 3 }
fn default_slippage_tolerance_percent() -> f64 { 0.25 }
fn default_jito_tip_percent() -> u64 { 20 }
fn default_hot_transaction_threshold() -> usize { 5 }
fn default_activity_window_secs() -> u64 { 120 }


impl Config {
    pub fn load() -> Result<Self> {
        dotenvy::dotenv().ok();
        let config = envy::from_env::<Config>()?;
        Ok(config)
    }

    pub fn payer_keypair(&self) -> Result<Keypair> {
        Ok(Keypair::from_base58_string(&self.payer_private_key))
    }
}

// Elle permet de parser une chaîne "val1,val2,val3" depuis .env en un Vec<String>
fn deserialize_string_list<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    struct StringListVisitor;

    impl<'de> Visitor<'de> for StringListVisitor {
        type Value = Vec<String>;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("a string of comma-separated values")
        }

        fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            // Filtre les chaînes vides au cas où il y aurait des virgules en trop (ex: "addr1,,addr2")
            Ok(value.split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect())
        }
    }

    deserializer.deserialize_str(StringListVisitor)
}