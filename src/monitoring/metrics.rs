// DANS : src/monitoring/metrics.rs

use lazy_static::lazy_static;
use prometheus::{
    Encoder, Histogram, HistogramVec, IntCounter, IntCounterVec, IntGauge, TextEncoder,
    register_histogram, register_histogram_vec, register_int_counter, register_int_counter_vec,
    register_int_gauge,
};
use warp::Filter;

lazy_static! {
    // --- Stratégie & Opportunités ---
    pub static ref OPPORTUNITIES_FOUND: IntCounter = register_int_counter!(
        "mev_opportunities_found_total", "Nombre total d'opportunités d'arbitrage trouvées par les stratégies"
    ).unwrap();
    pub static ref ARBITRAGE_PROFIT_ESTIMATED: Histogram = register_histogram!(
        "mev_arbitrage_profit_estimated_lamports", "Distribution des profits estimés des opportunités"
    ).unwrap();

    // --- Exécution & Résultats ---
    pub static ref TRANSACTIONS_SENT: IntCounter = register_int_counter!(
        "mev_transactions_sent_total", "Nombre de transactions envoyées (Jito ou Normal)"
    ).unwrap();

    // --- Performance & Latence (Pour les problèmes "invisibles") ---
    pub static ref PROCESS_OPPORTUNITY_LATENCY: Histogram = register_histogram!(
        "mev_process_opportunity_latency_seconds", "Latence du traitement complet d'une opportunité (de la détection à l'envoi/abandon)"
    ).unwrap();
    pub static ref RPC_REQUEST_LATENCY: HistogramVec = register_histogram_vec!(
        "mev_rpc_request_latency_seconds",
        "Latence des appels RPC vers le nœud Solana",
        &["method"] // Labels: "get_account", "simulate_transaction", etc.
    ).unwrap();
    pub static ref GET_QUOTE_LATENCY: HistogramVec = register_histogram_vec!(
        "mev_get_quote_latency_seconds",
        "Latence des calculs de get_quote",
        &["dex_type"] // Labels: "RaydiumCLMM", "OrcaWhirlpool", etc.
    ).unwrap();

    // --- Santé des Composants Internes ---
    pub static ref HOTLIST_POOL_COUNT: IntGauge = register_int_gauge!(
        "mev_hotlist_pool_count", "Nombre de pools actuellement dans la hotlist"
    ).unwrap();
    pub static ref GRAPH_POOL_COUNT: IntGauge = register_int_gauge!(
        "mev_graph_pool_count", "Nombre de pools dans le graphe d'arbitrage en mémoire"
    ).unwrap();

    pub static ref GEYSER_MESSAGES_RECEIVED: IntCounter = register_int_counter!(
        "mev_geyser_messages_received_total", "Nombre total de messages reçus du stream Geyser"
    ).unwrap();
    pub static ref GEYSER_LAST_MESSAGE_TIMESTAMP: IntGauge = register_int_gauge!(
        "mev_geyser_last_message_timestamp_seconds", "Timestamp Unix du dernier message reçu de Geyser"
    ).unwrap();

    pub static ref RPC_REQUESTS_TOTAL: IntCounterVec = register_int_counter_vec!(
        "mev_rpc_requests_total",
        "Compteur total des requêtes RPC, segmenté par méthode et statut",
        &["method", "status"] // Labels: "get_account", "success" / "failure"
    ).unwrap();

    pub static ref ARBITRAGE_PROFIT_REALIZED: Histogram = register_histogram!(
        "mev_arbitrage_profit_realized_lamports", "Distribution des profits réels (on-chain) des arbitrages réussis"
    ).unwrap();

    pub static ref ARBITRAGE_SLIPPAGE: Histogram = register_histogram!(
        "mev_arbitrage_slippage_lamports", "Distribution du slippage (différence entre profit estimé et réel)"
    ).unwrap();

    pub static ref TRANSACTION_OUTCOMES: IntCounterVec = register_int_counter_vec!(
        "mev_transaction_outcomes_total",
        "Résultats des transactions après décision",
        // --- MODIFICATION ICI ---
        &["outcome", "pool_pair"] // Labels: "SimSuccess", "pool1-pool2"
        // --- FIN DE LA MODIFICATION ---
    ).unwrap();

}

// La fonction start_metrics_server ne change pas
pub async fn start_metrics_server() {
    let metrics_route = warp::path!("metrics").map(|| {
        let encoder = TextEncoder::new();
        let mut buffer = vec![];
        encoder.encode(&prometheus::gather(), &mut buffer).unwrap();
        warp::reply::with_header(buffer, "content-type", "text/plain; version=0.0.4")
    });
    println!("[Monitoring] Serveur de métriques exposé sur http://0.0.0.0:9100/metrics");
    warp::serve(metrics_route).run(([0, 0, 0, 0], 9100)).await;
}