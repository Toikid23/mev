// src/data_pipeline/api_connectors/dexscreener.rs

use serde::Deserialize;
use anyhow::{Result, anyhow};
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;

#[derive(Debug, Deserialize)]
pub struct ApiResponse {
    pub pairs: Vec<Pair>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pair {
    pub chain_id: String,
    pub pair_address: String,
    pub base_token: Token,
    pub quote_token: Token,
    pub price_usd: Option<String>,
    #[serde(default)]
    pub volume: Volume,
    #[serde(default)]
    pub liquidity: Liquidity,

    // --- LA CORRECTION EST ICI ---
    #[serde(default)] // Dit à Serde de ne pas paniquer si le champ est manquant
    #[serde(deserialize_with = "optional_timestamp_from_i64")] // On utilise une nouvelle fonction
    pub pair_created_at: i64,
}


// ... (les structs Token, Volume, Liquidity et la fonction timestamp_from_i64 ne changent pas) ...
#[derive(Debug, Deserialize)]
pub struct Token {
    pub address: String,
    pub name: String,
    pub symbol: String,
}
#[derive(Debug, Deserialize, Default)]
pub struct Volume {
    #[serde(default)]
    pub h24: f64,
    #[serde(default)]
    pub h6: f64,
    #[serde(default)]
    pub h1: f64,
    #[serde(default)]
    pub m5: f64,
}
#[derive(Debug, Deserialize, Default)]
pub struct Liquidity {
    #[serde(default)]
    pub usd: f64,
}

fn optional_timestamp_from_i64<'de, D>(deserializer: D) -> Result<i64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    // On essaie de désérialiser en tant que Option<i64>
    let opt: Option<i64> = Deserialize::deserialize(deserializer)?;
    // Si c'est Some(ts), on divise par 1000. Si c'est None, on retourne 0.
    Ok(opt.map_or(0, |ts| ts / 1000))
}


// --- On crée une nouvelle structure pour notre sortie "propre" ---
#[derive(Debug, Clone)]
pub struct SolanaPair {
    pub address: Pubkey,
    pub base_token_address: Pubkey,
    pub quote_token_address: Pubkey,
    pub liquidity_usd: f64,
    pub volume_h24: f64,
    pub created_at: i64,
}

pub async fn search_and_filter_pairs(
    query: &str,
    min_liquidity_usd: f64,
    min_volume_h24: f64,
) -> Result<Vec<SolanaPair>> { // <-- La fonction retourne maintenant notre type propre
    let url = format!("https://api.dexscreener.com/latest/dex/search?q={}", query);
    let client = reqwest::Client::new();
    let response = client.get(&url).send().await?;

    if !response.status().is_success() {
        return Err(anyhow!("Erreur API DexScreener: {}", response.status()));
    }

    let response_text = response.text().await?;
    let api_response: ApiResponse = serde_json::from_str(&response_text)
        .map_err(|e| anyhow!("Erreur de décodage JSON: {}. Réponse reçue: {}", e, response_text))?;

    let filtered_pairs = api_response
        .pairs
        .into_iter()
        // --- ÉTAPE 1 : On ne garde que les paires de Solana ---
        .filter(|pair| pair.chain_id == "solana")
        // --- ÉTAPE 2 : On filtre par liquidité et volume ---
        .filter(|pair| {
            pair.liquidity.usd >= min_liquidity_usd && pair.volume.h24 >= min_volume_h24
        })
        // --- ÉTAPE 3 : On transforme la `Pair` brute en notre `SolanaPair` propre ---
        .filter_map(|pair| {
            // On essaie de parser les adresses, et on ignore la paire si une adresse est invalide
            let pair_address = Pubkey::from_str(&pair.pair_address).ok()?;
            let base_token_address = Pubkey::from_str(&pair.base_token.address).ok()?;
            let quote_token_address = Pubkey::from_str(&pair.quote_token.address).ok()?;

            Some(SolanaPair {
                address: pair_address,
                base_token_address,
                quote_token_address,
                liquidity_usd: pair.liquidity.usd,
                volume_h24: pair.volume.h24,
                created_at: pair.pair_created_at,
            })
        })
        .collect();

    Ok(filtered_pairs)
}