use borsh::BorshDeserialize;
use solana_sdk::pubkey::Pubkey;

#[derive(BorshDeserialize, Debug)]
pub struct PumpBuyEvent {
    pub timestamp: i64,
    pub base_amount_out: u64,
    pub max_quote_amount_in: u64,
    pub user_base_token_reserves: u64,
    pub user_quote_token_reserves: u64,
    pub pool_base_token_reserves: u64,
    pub pool_quote_token_reserves: u64,
    pub quote_amount_in: u64,
    pub lp_fee_basis_points: u64,
    pub lp_fee: u64,
    pub protocol_fee_basis_points: u64,
    pub protocol_fee: u64,
    pub quote_amount_in_with_lp_fee: u64,
    pub user_quote_amount_in: u64,
    pub pool: Pubkey,
    pub user: Pubkey,
    pub user_base_token_account: Pubkey,
    pub user_quote_token_account: Pubkey,
    pub protocol_fee_recipient: Pubkey,
    pub protocol_fee_recipient_token_account: Pubkey,
    pub coin_creator: Pubkey,
    pub coin_creator_fee_basis_points: u64,
    pub coin_creator_fee: u64,
}

#[derive(BorshDeserialize, Debug)]
pub struct PumpSellEvent {
    pub timestamp: i64,
    pub base_amount_in: u64,
    pub min_quote_amount_out: u64,
    pub user_base_token_reserves: u64,
    pub user_quote_token_reserves: u64,
    pub pool_base_token_reserves: u64,
    pub pool_quote_token_reserves: u64,
    pub quote_amount_out: u64,
    pub lp_fee_basis_points: u64,
    pub lp_fee: u64,
    pub protocol_fee_basis_points: u64,
    pub protocol_fee: u64,
    pub quote_amount_out_without_lp_fee: u64,
    pub user_quote_amount_out: u64,
    pub pool: Pubkey,
    pub user: Pubkey,
    pub user_base_token_account: Pubkey,
    pub user_quote_token_account: Pubkey,
    pub protocol_fee_recipient: Pubkey,
    pub protocol_fee_recipient_token_account: Pubkey,
    pub coin_creator: Pubkey,
    pub coin_creator_fee_basis_points: u64,
    pub coin_creator_fee: u64,
}

/// Tente de parser un `PumpBuyEvent` depuis les logs de simulation.
pub fn parse_pump_buy_event_from_logs(logs: &[String]) -> Option<u64> {
    const BUY_EVENT_DISCRIMINATOR: [u8; 8] = [103, 244, 82, 31, 44, 245, 119, 119];

    for log in logs {
        if let Some(data_str) = log.strip_prefix("Program data: ") {
            if let Ok(bytes) = base64::decode(data_str) {
                if bytes.len() > 8 && bytes.starts_with(&BUY_EVENT_DISCRIMINATOR) {
                    let event_data = &bytes[8..];
                    const OFFSET: usize = 8; // base_amount_out est à l'offset 8

                    if event_data.len() >= OFFSET + 8 {
                        let amount_bytes: [u8; 8] = event_data[OFFSET..OFFSET + 8].try_into().ok()?;
                        return Some(u64::from_le_bytes(amount_bytes));
                    }
                }
            }
        }
    }
    None
}

/// Tente de parser un `PumpSellEvent` depuis les logs de simulation.
pub fn parse_pump_sell_event_from_logs(logs: &[String]) -> Option<u64> {
    const SELL_EVENT_DISCRIMINATOR: [u8; 8] = [62, 47, 55, 10, 165, 3, 220, 42];

    for log in logs {
        if let Some(data_str) = log.strip_prefix("Program data: ") {
            if let Ok(bytes) = base64::decode(data_str) {
                if bytes.len() > 8 && bytes.starts_with(&SELL_EVENT_DISCRIMINATOR) {
                    let event_data = &bytes[8..];
                    const OFFSET: usize = 56; // quote_amount_out est à l'offset 56

                    if event_data.len() >= OFFSET + 8 {
                        let amount_bytes: [u8; 8] = event_data[OFFSET..OFFSET + 8].try_into().ok()?;
                        return Some(u64::from_le_bytes(amount_bytes));
                    }
                }
            }
        }
    }
    None
}