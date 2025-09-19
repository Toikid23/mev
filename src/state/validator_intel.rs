// DANS : src/state/validator_intel.rs (VERSION FINALE AVEC JITO FLAG)

use anyhow::{Context, Result};
use solana_sdk::pubkey::Pubkey;
use serde::Deserialize;
use std::{collections::HashMap, str::FromStr, sync::Arc, time::Duration};
use tokio::{sync::RwLock, task::JoinHandle};
use tracing::{error, info, warn, debug};

const VALIDATORS_APP_API_URL: &str = "https://www.validators.app/api/v1/validators/mainnet.json";
const REFRESH_INTERVAL_MINS: u64 = 60;

#[derive(Deserialize, Debug)]
struct ValidatorApiResponse {
    account: String,
    vote_account: String,
    data_center_key: Option<String>,
    active_stake: u64,
    skipped_slot_score: i32,
    jito: bool, // <-- ON AJOUTE LE CHAMP JITO
}

#[derive(Deserialize, Debug)]
struct ValidatorsAppResponse(Vec<ValidatorApiResponse>);

#[derive(Clone, Debug)]
pub struct ValidatorIntel {
    pub identity_pubkey: Pubkey,
    pub vote_pubkey: Pubkey,
    pub data_center_key: Option<String>,
    pub active_stake: u64,
    pub skipped_slot_score: i32,
    pub is_jito: bool,
}


#[derive(Clone)]
pub struct ValidatorIntelService {
    validators: Arc<RwLock<HashMap<Pubkey, ValidatorIntel>>>,
}

impl ValidatorIntelService {
    pub async fn new(api_token: String) -> Result<Self> {
        info!("Initialisation du ValidatorIntelService...");
        let service = Self {
            validators: Arc::new(RwLock::new(HashMap::new())),
        };
        service.refresh(&api_token).await?;
        info!("Initialisation du ValidatorIntelService réussie.");
        Ok(service)
    }

    pub fn start(&self, api_token: String) -> JoinHandle<()> {
        let self_clone = self.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(REFRESH_INTERVAL_MINS * 60));
            loop {
                interval.tick().await;
                info!("Rafraîchissement périodique des données des validateurs...");
                if let Err(e) = self_clone.refresh(&api_token).await {
                    error!(error = ?e, "Erreur lors du rafraîchissement des données des validateurs.");
                }
            }
        })
    }

    async fn refresh(&self, api_token: &str) -> Result<()> {
        let client = reqwest::Client::new();
        let response: ValidatorsAppResponse = client
            .get(VALIDATORS_APP_API_URL)
            .header("Token", api_token)
            .query(&[("limit", "9999")])
            .send()
            .await?
            .json()
            .await
            .context("Échec de la récupération ou du parsing des données de l'API validators.app")?;

        let new_validators: HashMap<Pubkey, ValidatorIntel> = response.0
            .into_iter()
            .filter_map(|v| {
                if let (Ok(identity_pubkey), Ok(vote_pubkey)) = (
                    Pubkey::from_str(&v.account),
                    Pubkey::from_str(&v.vote_account),
                ) {
                    Some((
                        identity_pubkey,
                        ValidatorIntel {
                            identity_pubkey,
                            vote_pubkey,
                            data_center_key: v.data_center_key,
                            active_stake: v.active_stake,
                            skipped_slot_score: v.skipped_slot_score,
                            is_jito: v.jito,
                        },
                    ))
                } else {
                    None
                }
            })
            .collect();

        let mut writer = self.validators.write().await;
        *writer = new_validators;

        let jito_count = writer.values().filter(|v| v.is_jito).count();
        info!(validator_count = writer.len(), jito_count, "Données des validateurs rafraîchies.");
        Ok(())
    }

    pub async fn get_validator_intel(&self, identity_pubkey: &Pubkey) -> Option<ValidatorIntel> {
        let reader = self.validators.read().await;
        reader.get(identity_pubkey).cloned()
    }
}