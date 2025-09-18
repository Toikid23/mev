
use serde::Deserialize;
use anyhow::Result;
use solana_sdk::signature::Keypair;

#[derive(Deserialize, Debug, Clone)]
pub struct Config {
    pub solana_rpc_url: String,
    pub payer_private_key: String,
    pub birdeye_api_key: String,
    #[serde(default = "default_min_profit_threshold")]
    pub min_profit_threshold: u64,
    #[serde(default = "default_max_cumulative_loss")]
    pub max_cumulative_loss: u64,

    // --- NOUVEAUX CHAMPS ---
    #[serde(default = "default_min_sol_balance")]
    pub min_sol_balance: u64,
    #[serde(default = "default_unwrap_amount")]
    pub unwrap_amount: u64,
    #[serde(default = "default_max_trade_size_sol")]
    pub max_trade_size_sol: f64,
    #[serde(default = "default_safety_margin_ms")]
    pub transaction_send_safety_margin_ms: u64,
}

fn default_safety_margin_ms() -> u64 { 50 } // Marge de 50ms par défaut

fn default_max_trade_size_sol() -> f64 {
    10.0 // Limite par défaut à 10 SOL par trade
}

fn default_min_profit_threshold() -> u64 {
    50000 // 0.00005 SOL
}

fn default_max_cumulative_loss() -> u64 {
    100_000_000 // 0.1 SOL
}

// --- NOUVELLES FONCTIONS DE VALEUR PAR DÉFAUT ---
fn default_min_sol_balance() -> u64 {
    // Seuil de déclenchement : 0.05 SOL
    50_000_000
}

fn default_unwrap_amount() -> u64 {
    // Montant à recharger : 0.05 SOL
    50_000_000
}


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