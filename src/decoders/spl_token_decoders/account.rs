use anyhow::Result;
use solana_sdk::pubkey::Pubkey;
use spl_token::state::Account as SplTokenAccount;
use solana_program_pack::Pack;
#[derive(Debug, Clone, PartialEq)]
pub struct DecodedSplAccount {
    pub mint: Pubkey,
    pub owner: Pubkey,
    pub amount: u64,
}
/// Décode les données brutes d'un compte de jeton SPL.
pub fn decode_account(data: &[u8]) -> Result<DecodedSplAccount> {
    let spl_account = SplTokenAccount::unpack(data)?;
    Ok(DecodedSplAccount {
        mint: spl_account.mint,
        owner: spl_account.owner,
        amount: spl_account.amount,
    })
}        