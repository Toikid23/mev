
use borsh::BorshDeserialize;
use solana_sdk::pubkey::Pubkey;

#[derive(BorshDeserialize, Debug)]
pub struct TradedEvent {
    pub whirlpool: Pubkey,
    pub a_to_b: bool,
    pub pre_sqrt_price: u128,
    pub post_sqrt_price: u128,
    pub input_amount: u64,
    pub output_amount: u64,
    pub input_transfer_fee: u64,
    pub output_transfer_fee: u64,
    pub lp_fee: u64,
    pub protocol_fee: u64,
}

/// Tente de parser un `TradedEvent` depuis les logs de simulation d'un swap Whirlpool.
pub fn parse_traded_event_from_logs(logs: &[String]) -> Option<u64> {
    const TRADED_EVENT_DISCRIMINATOR: [u8; 8] = [220, 175, 38, 190, 76, 174, 130, 107];

    for log in logs {
        if let Some(data_str) = log.strip_prefix("Program data: ") {
            if let Ok(bytes) = base64::decode(data_str) {
                if bytes.len() > 8 && bytes.starts_with(&TRADED_EVENT_DISCRIMINATOR) {
                    let mut event_data: &[u8] = &bytes[8..];
                    if let Ok(event) = TradedEvent::try_from_slice(&mut event_data) {
                        return Some(event.output_amount);
                    }
                }
            }
        }
    }
    None
}