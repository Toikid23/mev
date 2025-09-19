// DANS : src/monitoring/logging.rs
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::EnvFilter; // <-- AJOUTER CET IMPORT

pub fn setup_logging() {
    // On crée un filtre qui lit la variable RUST_LOG.
    // S'il n'est pas défini, il utilisera "info" par défaut.
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info"));

    tracing_subscriber::fmt()
        .json()
        .with_env_filter(filter) // <-- AJOUTER CETTE LIGNE
        .with_span_events(FmtSpan::CLOSE)
        .with_target(true)
        .init();
}