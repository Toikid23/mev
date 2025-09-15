// DANS : src/bin/dev_runner.rs

use anyhow::Result;
use std::time::{Duration, Instant};
use tokio::time::sleep;
use tracing::{error, info, instrument, warn};
use mev::monitoring;

/// Une fonction de travail simulée. L'attribut `#[instrument]`
/// indique à `tracing` de créer une "span" pour cette fonction.
/// Quand la fonction se termine, `tracing` enregistrera automatiquement
/// sa durée d'exécution.
#[instrument]
async fn perform_dev_run_cycle(iteration: u64) {
    info!(iteration, "Début du cycle de travail de simulation.");

    // Simule un travail qui prend entre 1 et 3 secondes
    let sleep_duration = Duration::from_millis(1000 + (iteration % 2000));
    sleep(sleep_duration).await;

    // Simule différents types de résultats pour générer différents niveaux de log
    if iteration % 10 == 0 {
        // Un log de niveau ERREUR, avec des champs structurés
        error!(
            iteration,
            error_code = 500,
            details = "Erreur de simulation volontaire",
            "Échec critique du cycle de test !"
        );
    } else if iteration % 3 == 0 {
        // Un log de niveau AVERTISSEMENT
        warn!(
            iteration,
            latency_ms = sleep_duration.as_millis(),
            "Le cycle de test a pris plus de temps que prévu."
        );
    } else {
        // Un log de niveau INFORMATION
        info!(iteration, "Cycle de travail de simulation terminé avec succès.");
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // Étape 1: Initialiser le logging. C'est la TOUTE première chose à faire.
    monitoring::logging::setup_logging();

    // Étape 2: Lancer le serveur de métriques en tâche de fond (dans un "thread" tokio).
    tokio::spawn(monitoring::metrics::start_metrics_server());

    // On utilise `info!` au lieu de `println!`
    info!("--- Lancement du Dev Runner (Mode Monitoring Test) ---");

    let mut iteration_count: u64 = 0;
    loop {
        let start_time = Instant::now();

        // Étape 3: Exécuter notre logique de test
        perform_dev_run_cycle(iteration_count).await;

        // Étape 4: Mettre à jour nos métriques Prometheus
        monitoring::metrics::DEV_RUNNER_ITERATIONS.inc(); // Incrémente le compteur
        monitoring::metrics::DEV_RUNNER_CYCLE_LATENCY.observe(start_time.elapsed().as_secs_f64()); // Enregistre la durée

        info!("Attente de 3 secondes avant le prochain cycle...");
        sleep(Duration::from_secs(3)).await;
        iteration_count += 1;
    }
}