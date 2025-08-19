/// Tente de parser le montant de sortie (`out_amount`) depuis les logs
/// d'une transaction de swap Raydium AMMv4.
pub fn parse_ammv4_output_amount_from_logs(logs: &[String]) -> Option<u64> {
    for log in logs {
        if let Some(data_str) = log.strip_prefix("Program log: ray_log: ") {
            if let Ok(bytes) = base64::decode(data_str) {
                // L'offset pour `out_amount` dans l'événement SwapBaseInLog est de 66.
                const OFFSET_TO_OUT_AMOUNT: usize = 66;

                if bytes.len() >= OFFSET_TO_OUT_AMOUNT + 8 {
                    let amount_bytes: [u8; 8] = bytes[OFFSET_TO_OUT_AMOUNT..OFFSET_TO_OUT_AMOUNT + 8].try_into().ok()?;
                    return Some(u64::from_le_bytes(amount_bytes));
                }
            }
        }
    }
    None
}