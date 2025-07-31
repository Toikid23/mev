// DANS: src/decoders/meteora_decoders/mod.rs

use solana_sdk::pubkey;

pub mod dlmm;
pub mod amm;

// On définit l'adresse du programme ici pour qu'elle soit accessible
// par tous les décodeurs Meteora.
pub const ID: pubkey::Pubkey = pubkey!("LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo");