// Code de test pour trouver l'adresse du BinArray
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;

// Remplacez ces valeurs
const POOL_ADDRESS: &str = "5rCf1DM8LjKTw4YqhnoLcngyZYeNnQqztScTogYHAS6";
const ACTIVE_BIN_ID: i32 = -4183;
const METEORA_PROGRAM_ID: &str = "LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo";
const MAX_BIN_PER_ARRAY: i32 = 70;
const BIN_ARRAY_SEED: &[u8] = b"bin_array";

fn main() {
    let pool_pubkey = Pubkey::from_str(POOL_ADDRESS).unwrap();
    let program_id = Pubkey::from_str(METEORA_PROGRAM_ID).unwrap();

    let bin_array_index = (ACTIVE_BIN_ID as i64 / MAX_BIN_PER_ARRAY as i64)
        - if ACTIVE_BIN_ID < 0 && ACTIVE_BIN_ID % MAX_BIN_PER_ARRAY != 0 { 1 } else { 0 };

    let (pda, _) = Pubkey::find_program_address(
        &[
            BIN_ARRAY_SEED,
            &pool_pubkey.to_bytes(),
            &bin_array_index.to_le_bytes(),
        ],
        &program_id,
    );

    println!("L'adresse du BinArray à vérifier est : {}", pda);
}