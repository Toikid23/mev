use anyhow::Result;
use solana_sdk::{instruction::Instruction, pubkey::Pubkey};
use async_trait::async_trait;

#[async_trait]
pub trait PoolOperations {

    fn get_mints(&self) -> (Pubkey, Pubkey);

    fn get_vaults(&self) -> (Pubkey, Pubkey);

    fn get_reserves(&self) -> (u64, u64);

    fn address(&self) -> Pubkey;

    fn get_quote(&self, token_in_mint: &Pubkey, amount_in: u64, current_timestamp: i64) -> Result<u64>;

    fn get_required_input(
        &mut self,
        token_out_mint: &Pubkey, // Le mint du token que l'on veut recevoir
        amount_out: u64,         // La quantité que l'on veut recevoir
        current_timestamp: i64
    ) -> Result<u64>;

    /// Met à jour les champs internes du pool à partir des données brutes d'un compte.
    /// Retourne un booléen indiquant si une ré-hydratation RPC (ex: pour les ticks) est nécessaire.
    fn update_from_account_data(&mut self, account_pubkey: &Pubkey, account_data: &[u8]) -> Result<()>;

    fn create_swap_instruction(
        &self,
        token_in_mint: &Pubkey,
        amount_in: u64,
        min_amount_out: u64,
        user_accounts: &UserSwapAccounts,
    ) -> Result<Instruction>;
}

// La struct reste inchangée
pub struct UserSwapAccounts {
    pub owner: Pubkey,
    pub source: Pubkey,
    pub destination: Pubkey,
}


pub(crate) fn find_input_by_binary_search<F>(
    mut quote_fn: F,
    amount_out: u64,
    initial_high_bound: u64,
) -> Result<u64>
where
    F: FnMut(u64) -> Result<u64>,
{
    let mut low_bound: u64 = 0;
    let mut high_bound: u64 = initial_high_bound;
    let mut best_guess: u64 = high_bound;

    // --- LA MODIFICATION EST ICI ---
    let mut iterations = 0;
    const MAX_ITERATIONS: u32 = 32; // Filet de sécurité contre les boucles infinies

    while low_bound <= high_bound && iterations < MAX_ITERATIONS {
        let guess_input = low_bound.saturating_add(high_bound) / 2;
        if guess_input == 0 {
            low_bound = 1;
            iterations += 1;
            continue;
        }

        match quote_fn(guess_input) {
            Ok(quote_output) if quote_output >= amount_out => {
                best_guess = guess_input;
                high_bound = guess_input.saturating_sub(1);
            }
            _ => {
                low_bound = guess_input.saturating_add(1);
            }
        }
        iterations += 1;
    }
    // --- FIN DE LA MODIFICATION ---

    Ok(best_guess.saturating_add(1))
}