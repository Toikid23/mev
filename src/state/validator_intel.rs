// DANS : src/state/validator_intel.rs (Version Simplifiée)

use anyhow::{Context, Result};
use solana_sdk::pubkey::Pubkey;
use serde::Deserialize;
use std::{collections::HashSet, str::FromStr, sync::Arc, time::Duration};
use tokio::{sync::RwLock, task::JoinHandle};

const JITO_VALIDATORS_API_URL: &str = "https://kobe.mainnet.jito.network/api/v1/validators";
const REFRESH_INTERVAL_MINS: u64 = 60; // Rafraîchir toutes les heures

#[derive(Deserialize, Debug)]
struct JitoValidator {
    vote_account: String,
    running_jito: bool,
}

#[derive(Deserialize, Debug)]
struct JitoApiResponse {
    validators: Vec<JitoValidator>,
}

/// Le service qui maintient la liste des VOTE ACCOUNTS des validateurs Jito à jour.
#[derive(Clone)]
pub struct ValidatorIntelService {
    jito_vote_accounts: Arc<RwLock<HashSet<Pubkey>>>,
}

impl ValidatorIntelService {
    /// Crée et initialise le service en faisant le premier appel à l'API Jito.
    pub async fn new() -> Result<Self> {
        println!("[ValidatorIntel] Initialisation...");
        let service = Self {
            jito_vote_accounts: Arc::new(RwLock::new(HashSet::new())),
        };
        service.refresh().await?;
        println!("[ValidatorIntel] Initialisation réussie.");
        Ok(service)
    }

    /// Lance la tâche de fond qui rafraîchit les données périodiquement.
    pub fn start(&self) -> JoinHandle<()> {
        let self_clone = self.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(REFRESH_INTERVAL_MINS * 60));
            loop {
                interval.tick().await;
                println!("[ValidatorIntel] Rafraîchissement périodique des validateurs Jito...");
                if let Err(e) = self_clone.refresh().await {
                    eprintln!("[ValidatorIntel] Erreur lors du rafraîchissement : {:?}", e);
                }
            }
        })
    }

    /// La logique de récupération et de mise à jour des données.
    async fn refresh(&self) -> Result<()> {
        let response: JitoApiResponse = reqwest::get(JITO_VALIDATORS_API_URL)
            .await?
            .json()
            .await
            .context("Échec de la récupération des données de l'API Jito")?;

        let new_jito_vote_accounts: HashSet<Pubkey> = response
            .validators
            .into_iter()
            .filter(|v| v.running_jito)
            .filter_map(|v| Pubkey::from_str(&v.vote_account).ok())
            .collect();

        let mut writer = self.jito_vote_accounts.write().await;
        *writer = new_jito_vote_accounts;

        println!("[ValidatorIntel] Données rafraîchies. {} validateurs Jito actifs en cache.", writer.len());
        Ok(())
    }

    /// Vérifie si un VOTE ACCOUNT donné est dans la liste Jito.
    pub async fn is_jito_validator(&self, vote_pubkey: &Pubkey) -> bool {
        let reader = self.jito_vote_accounts.read().await;
        reader.contains(vote_pubkey)
    }
}