// src/data_pipeline/api_connectors/birdeye.rs

use serde::Deserialize;
use anyhow::{Result, anyhow};
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;

// --- Structures pour la réponse V1 (validées) ---

#[derive(Debug, Deserialize)]
pub struct TokenListResponseV1 {
    pub data: TokenListDataV1,
    pub success: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenListDataV1 {
    pub tokens: Vec<TokenV1>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TokenV1 {
    #[serde(deserialize_with = "pubkey_from_string")]
    pub address: Pubkey,
    pub symbol: String,
    pub name: String,
    pub decimals: u8,
    #[serde(default)]
    pub liquidity: f64,
    #[serde(rename = "v24hUSD")]
    #[serde(default)]
    pub v24h_usd: f64,
    #[serde(rename = "v24hChangePercent")]
    pub v24h_change_percent: Option<f64>,
    #[serde(rename = "mc")]
    #[serde(default)]
    pub mc: f64,
}

fn pubkey_from_string<'de, D>(deserializer: D) -> Result<Pubkey, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s: &str = Deserialize::deserialize(deserializer)?;
    Pubkey::from_str(s).map_err(serde::de::Error::custom)
}

/// Récupère les tokens sur Solana en utilisant l'API V1 de Birdeye.
pub async fn get_top_tokens_v1(
    api_key: &str,
    offset: u32,
    limit: u32,
    min_liquidity: f64,
) -> Result<Vec<TokenV1>> {
    let url = "https://public-api.birdeye.so/defi/tokenlist";

    let client = reqwest::Client::new();
    let response = client.get(url)
        .header("x-api-key", api_key) // Header en minuscules
        .header("x-chain", "solana")   // Header pour spécifier la chain
        .query(&[
            ("offset", offset.to_string()),
            ("limit", limit.to_string()),
            ("sort_by", "v24hUSD".to_string()),
            ("sort_type", "desc".to_string()),
            ("min_liquidity", min_liquidity.to_string()),
        ])
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let error_body = response.text().await?;
        return Err(anyhow!("Erreur API Birdeye V1: {} - {}", status, error_body));
    }

    let response_text = response.text().await?;
    let response_json: TokenListResponseV1 = serde_json::from_str(&response_text)
        .map_err(|e| anyhow!("Erreur de décodage JSON V1: {}. Réponse reçue: {}", e, response_text))?;

    if !response_json.success {
        return Err(anyhow!("L'API Birdeye V1 a retourné success: false. Message: {}", response_text));
    }

    Ok(response_json.data.tokens)
}