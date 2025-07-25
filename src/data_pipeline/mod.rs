// src/data_pipeline/mod.rs

// On déclare les différents composants de notre pipeline de données.
// Rust saura trouver les fichiers et le sous-dossier correspondants.
pub mod discovery;
pub mod market_discovery;
pub mod rpc_data_scraper;
pub mod geyser_connector; // On le déclare déjà, il sera prêt pour le futur.