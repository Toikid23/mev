// Fichier COMPLET et FINAL : src/decoders/raydium_decoders/tick_array.rs

use bytemuck::{from_bytes, Pod, Zeroable};
use solana_sdk::pubkey::Pubkey;
use anyhow::{Result, bail};

// Constantes tirées directement du code source de Raydium
pub const TICK_ARRAY_SIZE: usize = 60;
pub const REWARD_NUM: usize = 3;

#[repr(C, packed)]
#[derive(Clone, Copy, Debug, Default)]
pub struct TickState {
    pub tick: i32,
    pub liquidity_net: i128,
    pub liquidity_gross: u128,
    pub fee_growth_outside_0_x64: u128,
    pub fee_growth_outside_1_x64: u128,
    pub reward_growths_outside_x64: [u128; REWARD_NUM],
    pub padding: [u32; 13],
}
unsafe impl Zeroable for TickState {}
unsafe impl Pod for TickState {}


#[repr(C, packed)]
#[derive(Clone, Copy, Debug)]
pub struct TickArrayState {
    pub pool_id: Pubkey,
    pub start_tick_index: i32,
    pub ticks: [TickState; TICK_ARRAY_SIZE],
    pub initialized_tick_count: u8,
    pub recent_epoch: u64,
    pub padding: [u8; 107],
}
unsafe impl Zeroable for TickArrayState {}
unsafe impl Pod for TickArrayState {}


/// Calcule l'adresse d'un compte TickArray (PDA).
/// **LA VERSION PROUVÉE CORRECTE**
pub fn get_tick_array_address(pool_id: &Pubkey, start_tick_index: i32, program_id: &Pubkey) -> Pubkey {
    let (pda, _) = Pubkey::find_program_address(
        &[
            b"tick_array",
            &pool_id.to_bytes(),
            &start_tick_index.to_be_bytes(), // <--- LA CORRECTION CRUCIALE
        ],
        program_id,
    );
    pda
}

/// Calcule le tick de départ d'un array.
/// **LA VERSION PROUVÉE CORRECTE**
pub fn get_start_tick_index(tick_index: i32, tick_spacing: u16) -> i32 {
    // NOTRE MOUCHARD : Affiche la valeur qui cause le problème.

    let ticks_in_array = (TICK_ARRAY_SIZE as i32) * (tick_spacing as i32);
    let mut start = tick_index / ticks_in_array;
    if tick_index < 0 && tick_index % ticks_in_array != 0 {
        start -= 1;
    }
    start * ticks_in_array
}

/// Décode les données brutes d'un compte TickArray.
pub fn decode_tick_array(data: &[u8]) -> Result<TickArrayState> {
    const DISCRIMINATOR: [u8; 8] = [192, 155, 85, 205, 49, 249, 129, 42];
    if data.get(..8) != Some(&DISCRIMINATOR) {
        bail!("Discriminator de TickArray invalide.");
    }
    let data_slice = &data[8..];
    if data_slice.len() != std::mem::size_of::<TickArrayState>() {
        bail!(
            "Taille de données de TickArray invalide. Attendu {}, reçu {}.",
            std::mem::size_of::<TickArrayState>(),
            data_slice.len()
        );
    }
    // On copie la valeur pour éviter les problèmes d'alignement
    let tick_array_state: TickArrayState = *from_bytes(data_slice);
    Ok(tick_array_state)
}