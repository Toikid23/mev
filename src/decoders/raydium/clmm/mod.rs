// 1. Déclarer tous les sous-modules pour les rendre accessibles
pub mod pool;
pub mod math;
pub mod tick_array;
pub mod tickarray_bitmap_extension;
pub mod events;
pub mod config;
pub mod full_math;
pub mod test;

// 2. Ré-exporter les éléments publics les plus importants
// Cela permet d'écrire `use crate::decoders::raydium_decoders::clmm::DecodedClmmPool`
// au lieu de `use crate::decoders::raydium_decoders::clmm::pool::DecodedClmmPool`
pub use pool::{decode_pool, hydrate, DecodedClmmPool};
pub use events::{parse_swap_event_from_logs, SwapEvent};