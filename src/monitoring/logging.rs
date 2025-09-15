use tracing_subscriber::fmt::format::FmtSpan;

pub fn setup_logging() {
    tracing_subscriber::fmt()
        .json() // Le point clé : formater la sortie en JSON structuré.
        .with_span_events(FmtSpan::CLOSE) // Permet de mesurer la durée des fonctions (spans).
        .with_target(true) // Inclut le chemin du module dans le log (ex: `mev::strategies::spatial`).
        .init(); // Active ce subscriber comme le logger global pour toute l'application.
}