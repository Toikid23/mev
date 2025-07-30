// DANS: src/decoders/orca_decoders/tick_array.rs

use bytemuck::{Pod, Zeroable};
use solana_sdk::pubkey::Pubkey;
use anyhow::{Result, ensure, anyhow};
use std::mem;

pub const TICK_ARRAY_SIZE: usize = 88;
pub const NUM_REWARDS: usize = 3;

#[repr(C, packed)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct TickData {
    pub initialized: u8, pub liquidity_net: i128, pub liquidity_gross: u128,
    pub fee_growth_outside_a: u128, pub fee_growth_outside_b: u128,
    pub reward_growths_outside: [u128; NUM_REWARDS],
}

#[repr(C, packed)]
#[derive(Clone, Copy, Debug)]
pub struct TickArrayData {
    pub start_tick_index: i32, pub ticks: [TickData; TICK_ARRAY_SIZE], pub whirlpool: Pubkey,
}

// LA FORMULE CORRECTE (identique à celle de votre décodeur Raydium CLMM)
pub fn get_start_tick_index(tick_index: i32, tick_spacing: u16) -> i32 {
    let ticks_in_array = (TICK_ARRAY_SIZE as i32) * (tick_spacing as i32);
    let mut start = tick_index / ticks_in_array;
    if tick_index < 0 && tick_index % ticks_in_array != 0 {
        start -= 1;
    }
    start * ticks_in_array
}

// LA MÉTHODE DE SEEDING CORRECTE (celle qui trouve les comptes)
pub fn get_tick_array_address(whirlpool: &Pubkey, start_tick_index: i32, program_id: &Pubkey) -> Pubkey {
    let (pda, _) = Pubkey::find_program_address(
        &[
            b"tick_array",
            whirlpool.as_ref(),
            start_tick_index.to_string().as_bytes(),
        ],
        program_id,
    );
    pda
}

// LA FONCTION DE DÉCODAGE AVEC L'INSPECTION DU DISCRIMINATEUR
pub fn decode_tick_array(data: &[u8]) -> Result<TickArrayData> {
    // LA CONSTANTE CORRECTE, PROUVÉE PAR NOTRE DÉBOGAGE
    const CORRECT_DISCRIMINATOR: [u8; 8] = [69, 97, 189, 190, 110, 7, 66, 187];
    ensure!(data.get(..8) == Some(&CORRECT_DISCRIMINATOR), "Invalid TickArray discriminator");

    let data_slice = &data[8..];
    let expected_size = mem::size_of::<TickArrayData>();
    ensure!(data_slice.len() >= expected_size, "TickArray data length mismatch");

    let mut offset = 0;

    let start_tick_bytes: [u8; 4] = data_slice[offset..offset + 4].try_into()?;
    let start_tick_index = i32::from_le_bytes(start_tick_bytes);
    offset += 4;

    let mut ticks = [TickData::default(); TICK_ARRAY_SIZE];
    let tick_data_size = mem::size_of::<TickData>();
    for i in 0..TICK_ARRAY_SIZE {
        let start = offset + i * tick_data_size;
        let end = start + tick_data_size;
        ticks[i] = *bytemuck::from_bytes(&data_slice[start..end]);
    }
    offset += tick_data_size * TICK_ARRAY_SIZE;

    let whirlpool_bytes: [u8; 32] = data_slice[offset..offset + 32].try_into()?;
    let whirlpool = Pubkey::new_from_array(whirlpool_bytes);

    Ok(TickArrayData { start_tick_index, ticks, whirlpool })
}