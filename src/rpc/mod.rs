// DANS : src/rpc/mod.rs
pub mod resilient_client;

// On ré-exporte le client pour un accès plus facile
pub use resilient_client::ResilientRpcClient;