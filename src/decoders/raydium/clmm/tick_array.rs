// Fichier : src/decoders/raydium_decoders/tick_array.rs
// STATUT : VERSION FINALE DE DÉBOGAGE ET DE CORRECTION

use bytemuck::{from_bytes, Pod, Zeroable};
use solana_sdk::pubkey::Pubkey;
use anyhow::{Result, bail};
use std::mem;
use serde::{Serialize, Deserialize};

pub const TICK_ARRAY_SIZE: usize = 60;
pub const REWARD_NUM: usize = 3;

#[repr(C, packed)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable, Serialize, Deserialize)]
pub struct TickState {
    pub tick: i32,
    pub liquidity_net: i128,
    pub liquidity_gross: u128,
    pub fee_growth_outside_0_x64: u128,
    pub fee_growth_outside_1_x64: u128,
    pub reward_growths_outside_x64: [u128; REWARD_NUM],
    pub padding: [u32; 13],
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct TickArrayState {
    pub pool_id: Pubkey,
    pub start_tick_index: i32,

    #[serde(with = "serde_arrays")]
    pub ticks: [TickState; TICK_ARRAY_SIZE],

    pub initialized_tick_count: u8,
    pub recent_epoch: u64,

    // --- AJOUTEZ L'ATTRIBUT ICI AUSSI ---
    #[serde(with = "serde_arrays")]
    pub padding: [u8; 107],
}
// LA MÉTHODE DE CALCUL DE PDA VALIDÉE PAR LE VÉRIFICATEUR : BIG-ENDIAN
pub fn get_tick_array_address(pool_id: &Pubkey, start_tick_index: i32, program_id: &Pubkey) -> Pubkey {
    let (pda, _) = Pubkey::find_program_address(
        &[
            b"tick_array",
            pool_id.as_ref(),
            &start_tick_index.to_be_bytes(),
        ],
        program_id,
    );
    pda
}

pub fn get_start_tick_index(tick_index: i32, tick_spacing: u16) -> i32 {
    let ticks_in_array = (TICK_ARRAY_SIZE as i32) * (tick_spacing as i32);
    // div_euclid est l'équivalent mathématique exact de Math.floor pour la division.
    // Il gère correctement les nombres positifs et négatifs.
    let start_index = tick_index.div_euclid(ticks_in_array);
    start_index * ticks_in_array
}

// LA MÉTHODE DE DÉCODAGE MANUELLE ET ROBUSTE
pub fn decode_tick_array(data: &[u8]) -> Result<TickArrayState> {


    const DISCRIMINATOR: [u8; 8] = [192, 155, 85, 205, 49, 249, 129, 42];

    // --- DÉBOGAGE DU DISCRIMINATEUR ---
    if data.len() >= 8 {
        let received_discriminator: [u8; 8] = data[..8].try_into().unwrap();
    }
    // --- FIN DU DÉBOGAGE ---

    if data.get(..8) != Some(&DISCRIMINATOR) {
        bail!("Discriminator de TickArray invalide.");
    }

    let data_slice = &data[8..];
    if data_slice.len() != mem::size_of::<TickArrayState>() {
        bail!(
            "Taille de données de TickArray invalide. Attendu {}, reçu {}.",
            mem::size_of::<TickArrayState>(),
            data_slice.len()
        );
    }

    // ... le reste de la fonction est correct
    let mut offset = 0;
    let pool_id_bytes: [u8; 32] = data_slice[offset..offset+32].try_into()?;
    let pool_id = Pubkey::new_from_array(pool_id_bytes);
    offset += 32;
    let start_tick_bytes: [u8; 4] = data_slice[offset..offset+4].try_into()?;
    let start_tick_index = i32::from_le_bytes(start_tick_bytes);
    offset += 4;
    let mut ticks = [TickState::default(); TICK_ARRAY_SIZE];
    let tick_data_size = mem::size_of::<TickState>();
    for i in 0..TICK_ARRAY_SIZE {
        let start = offset + i * tick_data_size;
        let end = start + tick_data_size;
        ticks[i] = *from_bytes(&data_slice[start..end]);
    }
    offset += tick_data_size * TICK_ARRAY_SIZE;
    let initialized_tick_count = data_slice[offset];
    offset += 1;
    let recent_epoch_bytes: [u8; 8] = data_slice[offset..offset+8].try_into()?;
    let recent_epoch = u64::from_le_bytes(recent_epoch_bytes);
    offset += 8;
    let padding: [u8; 107] = data_slice[offset..offset+107].try_into()?;
    Ok(TickArrayState {
        pool_id,
        start_tick_index,
        ticks,
        initialized_tick_count,
        recent_epoch,
        padding,
    })
}