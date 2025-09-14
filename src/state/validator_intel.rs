// DANS : src/state/validator_intel.rs

use anyhow::{Context, Result};
use solana_sdk::pubkey::Pubkey;
use serde::Deserialize;
use std::{collections::HashMap, str::FromStr, sync::Arc, time::Duration};
use tokio::{sync::RwLock, task::JoinHandle};

const VALIDATORS_APP_API_URL: &str = "https://www.validators.app/api/v1/validators/mainnet.json";
const REFRESH_INTERVAL_MINS: u64 = 60; // Rafraîchir toutes les heures

// Structure pour capturer tous les champs qui nous intéressent de l'API
#[derive(Deserialize, Debug)]
struct ValidatorApiResponse {
    account: String, // Identity Pubkey
    vote_account: String,
    jito: bool,
    data_center_key: Option<String>, // Contient la localisation comme "12345-US-Ashburn"
    active_stake: u64,
    skipped_slot_score: i32,
    skipped_after_score: i32,
    vote_latency_score: i32,
    // Ajoutez d'autres champs de l'API ici si vous en avez besoin
}

// La réponse de l'API est directement un tableau de ces objets.
#[derive(Deserialize, Debug)]
struct ValidatorsAppResponse(Vec<ValidatorApiResponse>);

// Notre structure de données interne, propre et optimisée pour notre bot.
#[derive(Clone, Debug)]
pub struct ValidatorInfo {
    pub identity_pubkey: Pubkey,
    pub vote_pubkey: Pubkey,
    pub location: String,
    pub active_stake: u64,
    pub skipped_slot_score: i32,
    // ... autres scores
}

/// Le service qui maintient la base de données des validateurs à jour.
#[derive(Clone)]
pub struct ValidatorIntelService {
    // Clé: Identity Pubkey. C'est plus direct car le LeaderSchedule nous donne l'identité.
    validators: Arc<RwLock<HashMap<Pubkey, ValidatorInfo>>>,
}

impl ValidatorIntelService {
    pub async fn new(api_token: String) -> Result<Self> {
        println!("[ValidatorIntel] Initialisation...");
        let service = Self {
            validators: Arc::new(RwLock::new(HashMap::new())),
        };
        service.refresh(&api_token).await?;
        println!("[ValidatorIntel] Initialisation réussie.");
        Ok(service)
    }

    pub fn start(&self, api_token: String) -> JoinHandle<()> {
        let self_clone = self.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(REFRESH_INTERVAL_MINS * 60));
            loop {
                interval.tick().await;
                println!("[ValidatorIntel] Rafraîchissement périodique des données des validateurs...");
                if let Err(e) = self_clone.refresh(&api_token).await {
                    eprintln!("[ValidatorIntel] Erreur lors du rafraîchissement : {:?}", e);
                }
            }
        })
    }

    async fn refresh(&self, api_token: &str) -> Result<()> {
        let client = reqwest::Client::new();
        let response: ValidatorsAppResponse = client
            .get(VALIDATORS_APP_API_URL)
            .header("Token", api_token) // Utilisation de la clé API
            .send()
            .await?
            .json()
            .await
            .context("Échec de la récupération ou du parsing des données de l'API validators.app")?;

        let new_validators: HashMap<Pubkey, ValidatorInfo> = response.0
            .into_iter()
            .filter_map(|v| {
                // On parse les deux clés, et on ne continue que si c'est valide
                if let (Ok(identity_pubkey), Ok(vote_pubkey)) = (
                    Pubkey::from_str(&v.account),
                    Pubkey::from_str(&v.vote_account),
                ) {
                    Some(
                        (identity_pubkey, ValidatorInfo {
                            identity_pubkey,
                            vote_pubkey,
                            // On extrait la ville depuis la data_center_key
                            location: v.data_center_key
                                .unwrap_or_else(|| "Unknown".to_string())
                                .split('-')
                                .nth(2)
                                .unwrap_or("Unknown")
                                .to_string(),
                            active_stake: v.active_stake,
                            skipped_slot_score: v.skipped_slot_score,
                        })
                    )
                } else {
                    None
                }
            })
            .collect();

        let mut writer = self.validators.write().await;
        *writer = new_validators;

        println!("[ValidatorIntel] Données rafraîchies. {} validateurs au total en cache.", writer.len());
        Ok(())
    }

    /// Retourne les informations sur un validateur via son IDENTITY pubkey.
    pub async fn get_validator_info(&self, identity_pubkey: &Pubkey) -> Option<ValidatorInfo> {
        let reader = self.validators.read().await;
        reader.get(identity_pubkey).cloned()
    }
}