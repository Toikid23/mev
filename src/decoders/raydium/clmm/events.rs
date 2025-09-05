use borsh::BorshDeserialize;
use solana_sdk::pubkey::Pubkey;
use base64::{engine::general_purpose::STANDARD, Engine as _};

#[derive(BorshDeserialize, Debug)]
pub struct SwapEvent {
    pub pool_state: Pubkey,
    pub sender: Pubkey,
    pub token_account_0: Pubkey,
    pub token_account_1: Pubkey,
    pub amount_0: u64,
    pub transfer_fee_0: u64,
    pub amount_1: u64,
    pub transfer_fee_1: u64,
    pub zero_for_one: bool,
    pub sqrt_price_x64: u128,
    pub liquidity: u128,
    pub tick: i32,
}

/// Tente de parser un `SwapEvent` depuis les logs de simulation d'une transaction CLMM.
pub fn parse_swap_event_from_logs(logs: &[String], is_base_input: bool) -> Option<u64> {
    // Discriminateur pour SwapEvent, tiré de l'IDL de Raydium
    const SWAP_EVENT_DISCRIMINATOR: [u8; 8] = [64, 198, 205, 232, 38, 8, 113, 226];

    for log in logs {
        if let Some(data_str) = log.strip_prefix("Program data: ") {
            if let Ok(bytes) = STANDARD.decode(data_str) {
                if bytes.len() > 8 && bytes.starts_with(&SWAP_EVENT_DISCRIMINATOR) {
                    let mut event_data: &[u8] = &bytes[8..];
                    if let Ok(event) = SwapEvent::try_from_slice(&mut event_data) {
                        // Si l'input est le token de base (A), la sortie est `amount_1` (token B)
                        // Sinon, la sortie est `amount_0` (token A)
                        return if is_base_input {
                            Some(event.amount_1)
                        } else {
                            Some(event.amount_0)
                        }
                    }
                }
            }
        }
    }
    None
}
