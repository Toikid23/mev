// src/decoders/raydium/cpmm/events.rs

use borsh::BorshDeserialize;
use solana_sdk::pubkey::Pubkey;

/// Représente la structure de l'événement de swap émis par le programme CPMM.
/// L'ordre des champs est crucial pour le décodage.
#[derive(BorshDeserialize, Debug)]
pub struct CpmmSwapEvent {
    pub pool_id: Pubkey,
    pub input_vault_before: u64,
    pub output_vault_before: u64,
    pub input_amount: u64,
    pub output_amount: u64, // <-- Le champ que nous voulons
    pub input_transfer_fee: u64,
    pub output_transfer_fee: u64,
    pub base_input: bool,
}

pub fn parse_output_amount_from_cpmm_event(logs: &[String]) -> Option<u64> {
    for log in logs {
        // Étape 1 : Trouver le bon log.
        if let Some(data_str) = log.strip_prefix("Program data: ") {
            // Étape 2 : Décoder la chaîne Base64.
            if let Ok(bytes) = base64::decode(data_str) {
                // Étape 3 : Définir l'offset correct.
                // D'après l'analyse des transactions CPMM sur Solscan, l'événement brut
                // contient plusieurs champs avant le montant de sortie. L'offset
                // pour `output_amount` est de 56 octets.
                // La structure est :
                // pool_id (32) + input_vault_before (8) + output_vault_before (8)
                // + input_amount (8) = 56.
                const OFFSET_TO_OUTPUT_AMOUNT: usize = 56;

                // Étape 4 : Extraire les 8 octets.
                if bytes.len() >= OFFSET_TO_OUTPUT_AMOUNT + 8 {
                    let amount_bytes: [u8; 8] = bytes[OFFSET_TO_OUTPUT_AMOUNT..OFFSET_TO_OUTPUT_AMOUNT + 8].try_into().ok()?;
                    // Étape 5 : Convertir en u64.
                    return Some(u64::from_le_bytes(amount_bytes));
                }
            }
        }
    }

    // Si on ne trouve aucun log correspondant, on retourne None.
    None
}