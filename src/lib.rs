// src/lib.rs

// On déclare tous nos modules principaux pour les rendre publics et
// utilisables par nos programmes binaires (main.rs, dev_runner.rs).
pub mod config;
pub mod data_pipeline;
pub mod decoders;
pub mod execution;
pub mod graph_engine;
pub mod state;
pub mod strategies;
pub mod filtering;
pub mod rpc;
pub mod monitoring;