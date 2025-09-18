pub mod quote_validator;
pub mod protection_calculator;
pub mod transaction_builder;
pub mod final_simulator;
use crate::execution::routing::JitoRegion;
use crate::decoders::Pool;
use crate::execution::protections::SwapProtections;
use crate::graph_engine::Graph;
use crate::rpc::ResilientRpcClient;
use crate::strategies::spatial::ArbitrageOpportunity;
use anyhow::Result;
use async_trait::async_trait;
use solana_sdk::signature::Keypair;
use solana_sdk::transaction::VersionedTransaction;
use std::sync::Arc;
use tracing::{info, error, Span};
use crate::config::Config;

/// Le Contexte est un objet qui transporte toutes les données nécessaires
/// à travers les différentes étapes (middlewares) du pipeline de traitement.
pub struct ExecutionContext {
    // Données initiales
    pub opportunity: ArbitrageOpportunity,
    pub graph_snapshot: Arc<Graph>,
    pub payer: Keypair,
    pub rpc_client: Arc<ResilientRpcClient>,
    pub current_timestamp: i64,

    // Données calculées par les middlewares
    pub pool_buy_from: Option<Pool>,
    pub pool_sell_to: Option<Pool>,
    pub estimated_profit: Option<u64>,
    pub intermediate_amount_out: Option<u64>,
    pub estimated_cus: Option<u64>,
    pub protections: Option<SwapProtections>,
    pub final_tx: Option<VersionedTransaction>,
    pub is_jito_leader: bool,
    pub jito_tip: Option<u64>,
    pub target_jito_region: Option<JitoRegion>,
    pub config: Config,

    // Métadonnées
    pub pool_pair_id: String,
    pub span: Span,
}

impl ExecutionContext {
    pub fn new(
        opportunity: ArbitrageOpportunity,
        graph_snapshot: Arc<Graph>,
        payer: Keypair,
        rpc_client: Arc<ResilientRpcClient>,
        current_timestamp: i64,
        span: Span,
        config: Config,
    ) -> Self {
        let mut pools = [opportunity.pool_buy_from_key.to_string(), opportunity.pool_sell_to_key.to_string()];
        pools.sort();
        let pool_pair_id = format!("{}-{}", pools[0], pools[1]);

        Self {
            opportunity,
            graph_snapshot,
            payer,
            rpc_client,
            current_timestamp,
            pool_buy_from: None,
            pool_sell_to: None,
            estimated_profit: None,
            intermediate_amount_out: None,
            estimated_cus: None,
            protections: None,
            final_tx: None,
            is_jito_leader: false,
            jito_tip: None,
            target_jito_region: None,
            config,
            pool_pair_id,
            span,
        }
    }
}

/// Un trait pour un Middleware. Chaque étape du pipeline implémentera ce trait.
#[async_trait]
pub trait Middleware: Send + Sync {
    /// Le nom du middleware, pour le logging.
    fn name(&self) -> &'static str;

    /// La fonction principale qui traite le contexte.
    /// Elle retourne `Ok(true)` pour continuer le pipeline, `Ok(false)` pour l'arrêter proprement,
    /// et `Err` en cas d'erreur irrécupérable.
    async fn process(&self, context: &mut ExecutionContext) -> Result<bool>;
}

/// Le Pipeline exécute une série de middlewares en séquence.
pub struct Pipeline {
    middlewares: Vec<Box<dyn Middleware>>,
}

impl Pipeline {
    pub fn new(middlewares: Vec<Box<dyn Middleware>>) -> Self { // <-- VÉRIFIEZ QUE LE CONSTRUCTEUR N'A QU'UN ARGUMENT
        Self { middlewares }
    }

    pub async fn run(&self, mut context: ExecutionContext) -> Result<()> {
        for middleware in &self.middlewares {
            let name = middleware.name();
            info!(middleware = name, "Exécution du middleware...");
            match middleware.process(&mut context).await {
                Ok(true) => {
                    // Continue
                }
                Ok(false) => {
                    // Arrêt propre, ce n'est PAS une erreur
                    info!(middleware = name, "Le middleware a arrêté le pipeline.");
                    context.span.record("outcome", format!("Stopped_at_{}", name));
                    return Ok(());
                }
                Err(e) => {
                    // Erreur critique, on la propage
                    error!(middleware = name, error = %e, "Erreur critique dans le middleware. Arrêt du pipeline.");
                    context.span.record("outcome", format!("Error_at_{}", name));
                    return Err(e); // On retourne l'erreur
                }
            }
        }
        Ok(()) // Si tout s'est bien passé
    }
}