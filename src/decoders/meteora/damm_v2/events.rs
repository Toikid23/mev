use borsh::BorshDeserialize;
use solana_sdk::pubkey::Pubkey;
use base64::{engine::general_purpose::STANDARD, Engine as _};

#[derive(BorshDeserialize, Debug)]
pub struct DammSwapResult {
    pub output_amount: u64,
    pub next_sqrt_price: u128,
    pub lp_fee: u64,
    pub protocol_fee: u64,
    pub partner_fee: u64,
    pub referral_fee: u64,
}

#[derive(BorshDeserialize, Debug)]
pub struct DammSwapEvent {
    pub pool: Pubkey,
    pub trade_direction: u8,
    pub has_referral: bool,
}

/// Tente de parser le montant de sortie (`output_amount`) depuis l'événement de swap
/// émis par le programme Meteora DAMM v2.
/// Cette fonction analyse manuellement les logs encodés en base64.
pub fn parse_damm_swap_event_from_logs(logs: &[String]) -> Option<u64> {
    // Discriminateur pour l'événement "EvtSwap", calculé depuis l'IDL: sha256("event:EvtSwap")[..8]
    const SWAP_EVENT_DISCRIMINATOR: [u8; 8] = [27, 60, 21, 213, 138, 170, 187, 147];

    for log in logs {
        if let Some(data_str) = log.strip_prefix("Program data: ") {
            if let Ok(bytes) = STANDARD.decode(data_str) {
                if bytes.len() > 8 && bytes.starts_with(&SWAP_EVENT_DISCRIMINATOR) {
                    let event_data = &bytes[8..];

                    // Offset de `output_amount` dans `swap_result`
                    // pool(32) + direction(1) + referral(1) + params.amount_in(8) + params.min_out(8) = 50
                    const OFFSET_TO_SWAP_RESULT: usize = 32 + 1 + 1 + 8 + 8;

                    if event_data.len() >= OFFSET_TO_SWAP_RESULT + 8 {
                        let amount_bytes: [u8; 8] = event_data[OFFSET_TO_SWAP_RESULT..OFFSET_TO_SWAP_RESULT + 8].try_into().ok()?;
                        return Some(u64::from_le_bytes(amount_bytes));
                    }
                }
            }
        }
    }
    None
}