use lazy_static::lazy_static;
use prometheus::{Encoder, Histogram, IntCounter, TextEncoder, register_histogram, register_int_counter};
use warp::Filter;

lazy_static! {
    // Déclare un compteur global et statique.
    // Il sera accessible de n'importe où dans le code.
    pub static ref DEV_RUNNER_ITERATIONS: IntCounter = register_int_counter!(
        "mev_dev_runner_iterations_total", // Le nom de la métrique dans Prometheus
        "Nombre d'itérations de la boucle de test de dev_runner" // Une description
    ).unwrap();

    // Déclare un histogramme global et statique.
    pub static ref DEV_RUNNER_CYCLE_LATENCY: Histogram = register_histogram!(
        "mev_dev_runner_cycle_latency_seconds",
        "Latence d'un cycle de travail de dev_runner en secondes",
        // Ce sont les "seaux" (buckets) pour classer les mesures de temps.
        // Ex: une mesure de 0.3s ira dans le seau "0.5".
        vec![0.1, 0.25, 0.5, 1.0, 2.5, 5.0]
    ).unwrap();
}

/// Démarre un serveur web léger sur le port 9100, qui ne répondra qu'à l'URL "/metrics".
pub async fn start_metrics_server() {
    let metrics_route = warp::path!("metrics").map(|| {
        let encoder = TextEncoder::new();
        let mut buffer = vec![];
        // Rassemble toutes les métriques enregistrées (nos compteurs, etc.)
        encoder.encode(&prometheus::gather(), &mut buffer).unwrap();
        // Retourne les métriques au format texte que Prometheus comprend.
        warp::reply::with_header(buffer, "content-type", "text/plain; version=0.0.4")
    });

    // Note: Ce println! est acceptable car il s'exécute une seule fois au démarrage
    // pour informer l'utilisateur.
    println!("[Monitoring] Serveur de métriques exposé sur http://0.0.0.0:9100/metrics");

    // Lance le serveur et le fait écouter sur toutes les interfaces réseau.
    warp::serve(metrics_route).run(([0, 0, 0, 0], 9100)).await;
}