#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

use tracing::{error, info, warn, debug};

fn main() {
    info!("--- MEV Scalpel Bot - Starting ---");
    // Ici, nous allons construire la logique d'orchestration du bot de production.
}