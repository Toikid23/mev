// src/execution/mod.rs

// On déclare les différents composants de la chaîne d'exécution.
pub mod optimizer;        // Pour calculer le montant optimal d'un trade.
pub mod simulate;         // Pour simuler les transactions avant envoi.
pub mod bundle_builder;   // Pour construire les bundles Jito.
pub mod bundle_sender;    // Pour envoyer les bundles aux Block Engines.