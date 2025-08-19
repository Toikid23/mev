use borsh::BorshDeserialize;

#[derive(BorshDeserialize, Debug)]
pub struct MeteoraSwapEvent {
    pub in_amount: u64,
    pub out_amount: u64,
    pub trade_fee: u64,
    pub protocol_fee: u64,
    pub host_fee: u64,
}

/// Tente de parser un `MeteoraSwapEvent` depuis les logs de simulation.
pub fn parse_meteora_swap_event_from_logs(logs: &[String]) -> Option<u64> {
    // Discriminateur pour l'événement "Swap", calculé depuis l'IDL: sha256("event:Swap")[..8]
    const SWAP_EVENT_DISCRIMINATOR: [u8; 8] = [81, 108, 227, 190, 205, 208, 10, 196];

    for log in logs {
        if let Some(data_str) = log.strip_prefix("Program data: ") {
            if let Ok(bytes) = base64::decode(data_str) {
                if bytes.len() > 8 && bytes.starts_with(&SWAP_EVENT_DISCRIMINATOR) {
                    let mut event_data: &[u8] = &bytes[8..];
                    if let Ok(event) = MeteoraSwapEvent::try_from_slice(&mut event_data) {
                        return Some(event.out_amount);
                    }
                }
            }
        }
    }
    None
}