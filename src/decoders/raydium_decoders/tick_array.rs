// src/decoders/raydium_decoders/tick_array.rs

use bytemuck::{from_bytes, Pod, Zeroable};
use solana_sdk::pubkey::Pubkey;
use anyhow::{Result, bail};

// D'après le code source, la taille est 88, pas 60.
pub const TICK_ARRAY_SIZE: usize = 88;

#[repr(C, packed)]
#[derive(Clone, Copy, Pod, Zeroable, Debug, Default)]
pub struct TickState {
    pub tick: i32,
    pub liquidity_net: i128,
    pub liquidity_gross: u128,
    pub fee_growth_outside_0_x64: u128,
    pub fee_growth_outside_1_x64: u128,
    pub reward_growths_outside_x64: [u128; 3],
}

#[repr(C, packed)]
#[derive(Clone, Copy, Debug)]
pub struct TickArrayState {
    pub pool_id: Pubkey,
    pub start_tick_index: i32,
    pub ticks: [TickState; TICK_ARRAY_SIZE], // 88 ticks * 96 bytes/tick = 8448 bytes
    // La structure est beaucoup plus petite en réalité.
    // L'IDL que nous avions était pour une version différente.
    // La VRAIE structure est beaucoup plus simple.
}
// Pour que la taille corresponde à 1832, la structure est en fait beaucoup plus simple.
// C'est la structure de la V2, pas de la V3.
// Oublions `bytemuck` pour ce compte et faisons une lecture manuelle.

/// Calcule l'adresse d'un compte TickArray.
pub fn get_tick_array_address(pool: &Pubkey, start_tick_index: i32, program_id: &Pubkey) -> Pubkey {
    let (pda, _) = Pubkey::find_program_address(
        &[
            b"tick_array",
            &pool.to_bytes(),
            &start_tick_index.to_le_bytes(),
        ],
        program_id,
    );
    pda
}

/// D'après l'analyse, la VRAIE structure d'un TickArray est la suivante.
pub struct DecodedTickArray {
    pub start_tick_index: i32,
    pub pool_id: Pubkey,
    // ... et une liste de ticks
}

pub fn decode_tick_array(data: &[u8]) -> Result<DecodedTickArray> {
    // 8 + 32 + 4 = 44 (début des ticks)
    let pool_id = Pubkey::new_from_array(data[8..40].try_into()?);
    let start_tick_index_bytes: [u8; 4] = data[40..44].try_into()?;
    let start_tick_index = i32::from_le_bytes(start_tick_index_bytes);

    Ok(DecodedTickArray {
        pool_id,
        start_tick_index,
    })
}