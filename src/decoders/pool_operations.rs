// src/decoders/pool_operations.rs

use solana_sdk::pubkey::Pubkey;
use anyhow::Result;

/// C'est notre "fiche résumé" standard, notre contrat.
/// N'importe quel type de pool que nous voudrons utiliser dans notre graphe
/// devra implémenter ce trait.
pub trait PoolOperations {
    /// Doit retourner les adresses des deux tokens du pool.
    fn get_mints(&self) -> (Pubkey, Pubkey);

    /// Doit retourner les adresses des deux vaults du pool.
    fn get_vaults(&self) -> (Pubkey, Pubkey);

    /// Doit calculer le montant de sortie pour un montant d'entrée donné.
    /// C'est la fonction la plus importante.
    fn get_quote(&self, token_in_mint: &Pubkey, amount_in: u64) -> Result<u64>;
}


