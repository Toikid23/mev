// DANS : src/config.rs

use serde::Deserialize;
use anyhow::Result;

#[derive(Deserialize, Debug)]
pub struct Config {
    pub solana_rpc_url: String,
    pub payer_private_key: String, // <-- AJOUTEZ CETTE LIGNE
}

impl Config {
    pub fn load() -> Result<Self> {
        dotenvy::dotenv().ok();
        let config = envy::from_env::<Config>()?;
        Ok(config)
    }
}