use borsh::BorshDeserialize;
use solana_sdk::pubkey::Pubkey;
use base64::{engine::general_purpose::STANDARD, Engine as _};

#[derive(BorshDeserialize, Debug)]
pub struct DlmmSwapEvent {
    pub lb_pair: Pubkey,
    pub from: Pubkey,
    pub start_bin_id: i32,
    pub end_bin_id: i32,
    pub amount_in: u64,
    pub amount_out: u64, // <-- Le champ qui nous intéresse
    pub swap_for_y: bool,
    pub fee: u64,
    pub protocol_fee: u64,
    pub fee_bps: u128,
    pub host_fee: u64,
}

/// Tente de parser un `DlmmSwapEvent` depuis les logs de simulation.
pub fn parse_dlmm_swap_event_from_logs(logs: &[String]) -> Option<u64> {
    // Discriminateur pour l'événement "Swap", tiré de l'IDL.
    const SWAP_EVENT_DISCRIMINATOR: [u8; 8] = [81, 108, 227, 190, 205, 208, 10, 196];

    for log in logs {
        if let Some(data_str) = log.strip_prefix("Program data: ") {
            if let Ok(bytes) = STANDARD.decode(data_str) {
                if bytes.len() > 8 && bytes.starts_with(&SWAP_EVENT_DISCRIMINATOR) {
                    let event_data: &[u8] = &bytes[8..];
                    if let Ok(event) = DlmmSwapEvent::try_from_slice(event_data) {
                        return Some(event.amount_out);
                    }
                }
            }
        }
    }
    None
}