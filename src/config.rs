// DANS : src/config.rs

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
}

// --- LA FONCTION MANQUANTE EST ICI ---
fn default_min_profit_threshold() -> u64 {
    50000 // Notre valeur par défaut sûre
}

fn default_max_cumulative_loss() -> u64 {
    100_000_000 // 0.1 SOL par défaut
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