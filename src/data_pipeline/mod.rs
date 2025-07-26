// src/data_pipeline/mod.rs

// On déclare les différents composants de notre pipeline de données.
// Rust saura trouver les fichiers et le sous-dossier correspondants.
pub mod unified_discovery;
pub mod geyser_connector;
mod dex_implementations;
pub mod onchain_scanner;
// On le déclare déjà, il sera prêt pour le futur.