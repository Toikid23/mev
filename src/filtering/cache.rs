// DANS : src/filtering/cache.rs

use anyhow::{Result, Context};
use solana_sdk::pubkey::Pubkey;
use std::{
    collections::HashMap,
    fs::File,
    io::{BufReader, BufWriter},
    path::Path,
};
use tracing::{error, info, warn, debug};
use super::PoolIdentity; // On importe la structure depuis notre mod.rs parent

const CACHE_FILE_NAME: &str = "pools_universe.json";

/// La structure principale du cache, optimisée pour des recherches rapides.
/// Elle sera chargée en mémoire au démarrage du bot de filtrage.
pub struct PoolCache {
    /// Permet de retrouver rapidement une `PoolIdentity` par son adresse de pool.
    pub pools: HashMap<Pubkey, PoolIdentity>,

    /// Permet de retrouver l'adresse d'un pool à partir d'un de ses comptes surveillés.
    /// C'est la structure clé pour notre analyseur Geyser.
    /// Clé: `account_to_watch` (ex: un vault), Valeur: `pool_address`.
    pub watch_map: HashMap<Pubkey, Pubkey>,
}

impl PoolCache {
    /// Charge la "bibliothèque" de pools depuis le fichier JSON sur le disque.
    /// Si le fichier n'existe pas, retourne un cache vide.
    pub fn load() -> Result<Self> {
        if !Path::new(CACHE_FILE_NAME).exists() {
            info!(file = CACHE_FILE_NAME, "Fichier de cache non trouvé. Démarrage avec un cache vide.");
            return Ok(Self {
                pools: HashMap::new(),
                watch_map: HashMap::new(),
            });
        }

        info!(file = CACHE_FILE_NAME, "Chargement de la bibliothèque de pools depuis le cache.");
        let file = File::open(CACHE_FILE_NAME)
            .with_context(|| format!("Impossible d'ouvrir le fichier de cache '{}'", CACHE_FILE_NAME))?;
        let reader = BufReader::new(file);

        let identities: Vec<PoolIdentity> = serde_json::from_reader(reader)
            .with_context(|| format!("Erreur de désérialisation du fichier de cache '{}'", CACHE_FILE_NAME))?;

        info!(pool_count = identities.len(), "Pools chargés. Construction des maps de recherche...");

        let mut pools = HashMap::with_capacity(identities.len());
        let mut watch_map = HashMap::new();

        for identity in identities {
            for watch_account in &identity.accounts_to_watch {
                watch_map.insert(*watch_account, identity.address);
            }
            pools.insert(identity.address, identity);
        }

        info!(pool_count = pools.len(), watch_count = watch_map.len(), "Cache prêt.");

        Ok(Self { pools, watch_map })
    }

    /// Sauvegarde la liste complète des identités de pools dans le fichier JSON.
    /// Cette fonction sera appelée par notre futur module "Recensement".
    pub fn save(identities: &[PoolIdentity]) -> Result<()> {
        info!(identity_count = identities.len(), file = CACHE_FILE_NAME, "Sauvegarde des identités de pools.");
        let file = File::create(CACHE_FILE_NAME)
            .with_context(|| format!("Impossible de créer le fichier de cache '{}'", CACHE_FILE_NAME))?;
        let writer = BufWriter::new(file);

        serde_json::to_writer_pretty(writer, identities)
            .with_context(|| format!("Erreur de sérialisation vers le fichier de cache '{}'", CACHE_FILE_NAME))?;

        info!("Sauvegarde du cache terminée avec succès.");
        Ok(())
    }
}