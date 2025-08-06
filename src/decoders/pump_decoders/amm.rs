// DANS: src/decoders/pump_decoders/amm.rs

use crate::decoders::pool_operations::PoolOperations;
use bytemuck::{Pod, Zeroable};
use solana_sdk::pubkey::Pubkey;
use anyhow::{anyhow, bail, Result};
use solana_client::nonblocking::rpc_client::RpcClient;
use crate::decoders::spl_token_decoders;

// --- CONSTANTES DU PROTOCOLE ---
// Trouvées dans l'IDL
pub const PUMP_PROGRAM_ID: Pubkey = solana_sdk::pubkey!("pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA");
const POOL_ACCOUNT_DISCRIMINATOR: [u8; 8] = [241, 154, 109, 4, 17, 177, 109, 188];
const GLOBAL_CONFIG_ACCOUNT_DISCRIMINATOR: [u8; 8] = [149, 8, 156, 202, 160, 252, 176, 217];


// --- STRUCTURE DE SORTIE "PROPRE" ---
// C'est notre format de travail interne, unifié et prêt pour le `PoolOperations`.
// Il sera rempli par les fonctions `decode_pool` et `hydrate`.
#[derive(Debug, Clone)]
pub struct DecodedPumpAmmPool {
    pub address: Pubkey,
    pub mint_a: Pubkey, // Le "base_mint" (le token qui est lancé)
    pub mint_b: Pubkey, // Le "quote_mint" (généralement SOL)
    pub vault_a: Pubkey,
    pub vault_b: Pubkey,

    // Champs à hydrater
    pub reserve_a: u64,
    pub reserve_b: u64,
    pub mint_a_decimals: u8,
    pub mint_b_decimals: u8,
    pub mint_a_transfer_fee_bps: u16,
    pub mint_b_transfer_fee_bps: u16,

    // Frais (hydratés depuis GlobalConfig)
    pub lp_fee_basis_points: u64,
    pub protocol_fee_basis_points: u64,
    pub coin_creator_fee_basis_points: u64,
    pub total_fee_basis_points: u64,
}

impl DecodedPumpAmmPool {
    /// Retourne le total des frais de pool en pourcentage.
    pub fn fee_as_percent(&self) -> f64 {
        (self.total_fee_basis_points as f64 / 10_000.0) * 100.0
    }
}


// --- MODULE PRIVÉ POUR LES STRUCTURES ON-CHAIN ---
// Contient les miroirs exacts des comptes de la blockchain.
// Pilier d'Excellence #6 : Robuste aux problèmes de layout mémoire.
mod onchain_layouts {
    use super::*;

    #[repr(C, packed)]
    #[derive(Clone, Copy, Pod, Zeroable, Debug)]
    pub struct Pool {
        pub pool_bump: u8,
        pub index: u16,
        pub creator: Pubkey,
        pub base_mint: Pubkey,
        pub quote_mint: Pubkey,
        pub lp_mint: Pubkey,
        pub pool_base_token_account: Pubkey,
        pub pool_quote_token_account: Pubkey,
        pub lp_supply: u64,
        pub coin_creator: Pubkey,
    }

    #[repr(C, packed)]
    #[derive(Clone, Copy, Pod, Zeroable, Debug)]
    pub struct GlobalConfig {
        pub admin: Pubkey,
        pub lp_fee_basis_points: u64,
        pub protocol_fee_basis_points: u64,
        pub disable_flags: u8,
        pub protocol_fee_recipients: [Pubkey; 8],
        pub coin_creator_fee_basis_points: u64,
        pub admin_set_coin_creator_authority: Pubkey,
    }
}


/// Tente de décoder les données brutes d'un compte Pool `pump.fun`.
/// Cette fonction ne fait que la lecture initiale et ne remplit pas les réserves ni les frais.
/// L'hydratation est gérée par la fonction `hydrate`.
pub fn decode_pool(address: &Pubkey, data: &[u8]) -> Result<DecodedPumpAmmPool> {
    // --- Pilier d'Excellence #6 : Robustesse du décodage ---

    // 1. Vérifier le discriminateur pour s'assurer que c'est le bon type de compte.
    if data.get(..8) != Some(&POOL_ACCOUNT_DISCRIMINATOR) {
        bail!("Invalid discriminator. Not a pump.fun Pool account.");
    }

    // 2. On ignore les 8 bytes du discriminateur.
    let data_slice = &data[8..];

    // 3. Vérifier que la taille des données restantes correspond à notre struct on-chain.
    if data_slice.len() < std::mem::size_of::<onchain_layouts::Pool>() {
        bail!(
            "Pump.fun Pool data length mismatch. Expected at least {} bytes, got {}.",
            std::mem::size_of::<onchain_layouts::Pool>(),
            data_slice.len()
        );
    }

    // 4. "Caster" les données brutes vers notre struct Rust via bytemuck pour une performance maximale.
    let pool_struct: &onchain_layouts::Pool = bytemuck::from_bytes(
        &data_slice[..std::mem::size_of::<onchain_layouts::Pool>()]
    );

    // 5. Créer et retourner notre structure "propre", en initialisant les champs à hydrater à zéro.
    Ok(DecodedPumpAmmPool {
        address: *address,
        mint_a: pool_struct.base_mint,
        mint_b: pool_struct.quote_mint,
        vault_a: pool_struct.pool_base_token_account,
        vault_b: pool_struct.pool_quote_token_account,

        // Tous les champs ci-dessous seront remplis par `hydrate`.
        reserve_a: 0,
        reserve_b: 0,
        mint_a_decimals: 0,
        mint_b_decimals: 0,
        mint_a_transfer_fee_bps: 0,
        mint_b_transfer_fee_bps: 0,
        lp_fee_basis_points: 0,
        protocol_fee_basis_points: 0,
        coin_creator_fee_basis_points: 0,
        total_fee_basis_points: 0,
    })
}



/// Remplit les informations manquantes du pool (réserves, frais) en effectuant des appels RPC.
/// Utilise `get_multiple_accounts` pour une efficacité maximale (Pilier #4).
pub async fn hydrate(pool: &mut DecodedPumpAmmPool, rpc_client: &RpcClient) -> Result<()> {
    // 1. Calculer l'adresse du compte de configuration globale (PDA).
    // Cette adresse est déterministe et la même pour tous les pools du programme.
    let (global_config_address, _) = Pubkey::find_program_address(
        &[b"global_config"],
        &PUMP_PROGRAM_ID
    );

    // 2. Préparer un seul appel groupé pour toutes les données nécessaires.
    let accounts_to_fetch = vec![
        pool.vault_a,
        pool.vault_b,
        pool.mint_a,
        pool.mint_b,
        global_config_address,
    ];

    let accounts_data = rpc_client.get_multiple_accounts(&accounts_to_fetch).await?;

    // --- TRAITEMENT DES RÉSULTATS ---

    // 3. Extraire et décoder les données de chaque compte.

    // Vault A (Base)
    let vault_a_data = accounts_data[0].as_ref().ok_or_else(|| anyhow!("Vault A not found for pump.fun pool {}", pool.address))?.data.clone();
    pool.reserve_a = u64::from_le_bytes(vault_a_data[64..72].try_into()?);

    // Vault B (Quote)
    let vault_b_data = accounts_data[1].as_ref().ok_or_else(|| anyhow!("Vault B not found for pump.fun pool {}", pool.address))?.data.clone();
    pool.reserve_b = u64::from_le_bytes(vault_b_data[64..72].try_into()?);

    // Mint A (Base) - On réutilise notre décodeur SPL existant.
    let mint_a_data = accounts_data[2].as_ref().ok_or_else(|| anyhow!("Mint A not found for pump.fun pool {}", pool.address))?.data.clone();
    let decoded_mint_a = spl_token_decoders::mint::decode_mint(&pool.mint_a, &mint_a_data)?;
    pool.mint_a_decimals = decoded_mint_a.decimals;
    pool.mint_a_transfer_fee_bps = decoded_mint_a.transfer_fee_basis_points;

    // Mint B (Quote)
    let mint_b_data = accounts_data[3].as_ref().ok_or_else(|| anyhow!("Mint B not found for pump.fun pool {}", pool.address))?.data.clone();
    let decoded_mint_b = spl_token_decoders::mint::decode_mint(&pool.mint_b, &mint_b_data)?;
    pool.mint_b_decimals = decoded_mint_b.decimals;
    pool.mint_b_transfer_fee_bps = decoded_mint_b.transfer_fee_basis_points;

    // GlobalConfig pour les Frais (Pilier #3)
    let global_config_data = accounts_data[4].as_ref().ok_or_else(|| anyhow!("GlobalConfig not found for pump.fun program"))?.data.clone();

    if global_config_data.get(..8) != Some(&GLOBAL_CONFIG_ACCOUNT_DISCRIMINATOR) {
        bail!("Invalid discriminator for pump.fun GlobalConfig account");
    }

    let config_data_slice = &global_config_data[8..];
    let config_struct: &onchain_layouts::GlobalConfig = bytemuck::from_bytes(
        &config_data_slice[..std::mem::size_of::<onchain_layouts::GlobalConfig>()]
    );

    pool.lp_fee_basis_points = config_struct.lp_fee_basis_points;
    pool.protocol_fee_basis_points = config_struct.protocol_fee_basis_points;
    pool.coin_creator_fee_basis_points = config_struct.coin_creator_fee_basis_points;

    // On somme les frais pour un accès simplifié plus tard.
    pool.total_fee_basis_points = pool.lp_fee_basis_points
        .saturating_add(pool.protocol_fee_basis_points)
        .saturating_add(pool.coin_creator_fee_basis_points);

    Ok(())
}


fn ceil_div(a: u128, b: u128) -> Option<u128> {
    if b == 0 { return None; }
    a.checked_add(b)?.checked_sub(1)?.checked_div(b)
}


impl PoolOperations for DecodedPumpAmmPool {
    /// Retourne les adresses des deux tokens du pool.
    fn get_mints(&self) -> (Pubkey, Pubkey) {
        (self.mint_a, self.mint_b)
    }

    /// Retourne les adresses des deux vaults du pool.
    fn get_vaults(&self) -> (Pubkey, Pubkey) {
        (self.vault_a, self.vault_b)
    }

    /// Calcule le montant de sortie pour un montant d'entrée donné.
    /// C'est la fonction la plus importante, elle doit être parfaite.
    fn get_quote(&self, token_in_mint: &Pubkey, amount_in: u64, _current_timestamp: i64) -> Result<u64> {
        // --- 1. Validation et Configuration Initiale ---
        if self.total_fee_basis_points == 0 && self.lp_fee_basis_points == 0 {
            return Err(anyhow!("Pool is not hydrated, fees are unknown."));
        }
        if self.reserve_a == 0 || self.reserve_b == 0 { return Ok(0); }

        let is_buy = *token_in_mint == self.mint_b; // Achat si l'input est le quote token (SOL)

        let (in_reserve, out_reserve, in_mint_fee_bps, out_mint_fee_bps) = if is_buy {
            (self.reserve_b, self.reserve_a, self.mint_b_transfer_fee_bps, self.mint_a_transfer_fee_bps)
        } else {
            (self.reserve_a, self.reserve_b, self.mint_a_transfer_fee_bps, self.mint_b_transfer_fee_bps)
        };

        // --- 2. Application des Frais de Transfert (Token-2022) sur l'INPUT ---
        let fee_on_input = (amount_in as u128 * in_mint_fee_bps as u128) / 10_000;
        let amount_in_after_transfer_fee = amount_in.saturating_sub(fee_on_input as u64);

        let amount_in_u128 = amount_in_after_transfer_fee as u128;
        let in_reserve_u128 = in_reserve as u128;
        let out_reserve_u128 = out_reserve as u128;

        // --- 3. Calcul du Swap Brut et des Frais de Pool (Logique 1:1 avec le SDK) ---
        let amount_out_after_pool_fee = if is_buy {
            // Logique d'ACHAT : Les frais sont sur l'INPUT. Le swap se fait sur le montant NET.
            let total_fee = (amount_in_u128 * self.total_fee_basis_points as u128) / 10_000;
            let net_amount_in = amount_in_u128.saturating_sub(total_fee);
            let new_numerator = net_amount_in * out_reserve_u128;
            let new_denominator = in_reserve_u128 + net_amount_in;
            if new_denominator == 0 { 0 } else { new_numerator / new_denominator }
        } else {
            // Logique de VENTE : Le swap se fait sur le montant BRUT, les frais sont sur l'OUTPUT.
            let numerator = amount_in_u128 * out_reserve_u128;
            let denominator = in_reserve_u128 + amount_in_u128;
            if denominator == 0 { return Ok(0); }

            let gross_amount_out = numerator / denominator;
            let total_fee = (gross_amount_out * self.total_fee_basis_points as u128) / 10_000;
            gross_amount_out.saturating_sub(total_fee)
        };

        // --- 4. Application des Frais de Transfert (Token-2022) sur l'OUTPUT ---
        let fee_on_output = (amount_out_after_pool_fee * out_mint_fee_bps as u128) / 10_000;
        let final_amount_out = amount_out_after_pool_fee.saturating_sub(fee_on_output);

        Ok(final_amount_out as u64)
    }
}