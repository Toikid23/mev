// DANS: src/decoders/pump_decoders/amm.rs

use crate::decoders::pool_operations::PoolOperations;
use bytemuck::{Pod, Zeroable};
use solana_sdk::pubkey::Pubkey;
use anyhow::{anyhow, bail, Result};
use solana_client::nonblocking::rpc_client::RpcClient;
use crate::decoders::spl_token_decoders;
use solana_sdk::instruction::{Instruction, AccountMeta};
use solana_sdk::system_program; // On aura besoin du system_program
use spl_associated_token_account::get_associated_token_address; // Pour trouver les ATA
use spl_associated_token_account::get_associated_token_address_with_program_id;

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
    pub coin_creator: Pubkey,
    pub protocol_fee_recipients: [Pubkey; 8],

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
    pub mint_a_program: Pubkey,
    pub mint_b_program: Pubkey,
}

impl DecodedPumpAmmPool {
    /// Retourne le total des frais de pool en pourcentage.
    pub fn fee_as_percent(&self) -> f64 {
        (self.total_fee_basis_points as f64 / 10_000.0) * 100.0
    }

    pub fn create_swap_instruction(
        &self,
        input_token_mint: &Pubkey,
        user_owner: &Pubkey,
        amount_in: u64,
        amount_out_from_quote: u64,
    ) -> Result<Instruction> {
        // ... (la logique des `instruction_data` et la dérivation des comptes est correcte et ne change pas) ...
        let is_buy = *input_token_mint == self.mint_b;
        let mut instruction_data = Vec::with_capacity(8 + 8 + 8);
        if is_buy {
            let discriminator: [u8; 8] = [102, 6, 61, 18, 1, 218, 235, 234];
            instruction_data.extend_from_slice(&discriminator);
            instruction_data.extend_from_slice(&amount_out_from_quote.to_le_bytes());
            instruction_data.extend_from_slice(&amount_in.to_le_bytes());
        } else {
            let discriminator: [u8; 8] = [51, 230, 133, 164, 1, 127, 131, 173];
            instruction_data.extend_from_slice(&discriminator);
            instruction_data.extend_from_slice(&amount_in.to_le_bytes());
            instruction_data.extend_from_slice(&amount_out_from_quote.to_le_bytes());
        };
        let (user_source_ata, user_destination_ata) = if is_buy {
            (
                get_associated_token_address_with_program_id(user_owner, &self.mint_b, &self.mint_b_program),
                get_associated_token_address_with_program_id(user_owner, &self.mint_a, &self.mint_a_program),
            )
        } else {
            (
                get_associated_token_address_with_program_id(user_owner, &self.mint_a, &self.mint_a_program),
                get_associated_token_address_with_program_id(user_owner, &self.mint_b, &self.mint_b_program),
            )
        };
        let (global_config_address, _) = Pubkey::find_program_address(&[b"global_config"], &PUMP_PROGRAM_ID);
        let (event_authority, _) = Pubkey::find_program_address(&[b"__event_authority"], &PUMP_PROGRAM_ID);
        let protocol_fee_recipient = self.protocol_fee_recipients.iter().find(|&&key| key != Pubkey::default()).unwrap_or(&self.protocol_fee_recipients[0]);
        let protocol_fee_recipient_token_account = get_associated_token_address_with_program_id(protocol_fee_recipient, &self.mint_b, &self.mint_b_program);
        let (coin_creator_vault_authority, _) = Pubkey::find_program_address(&[b"creator_vault", self.coin_creator.as_ref()], &PUMP_PROGRAM_ID);
        let coin_creator_vault_ata = get_associated_token_address_with_program_id(&coin_creator_vault_authority, &self.mint_b, &self.mint_b_program);
        let (global_volume_accumulator, _) = Pubkey::find_program_address(&[b"global_volume_accumulator"], &PUMP_PROGRAM_ID);
        let (user_volume_accumulator, _) = Pubkey::find_program_address(&[b"user_volume_accumulator", user_owner.as_ref()], &PUMP_PROGRAM_ID);

        // --- CORRECTION FINALE : Réorganisation des comptes pour correspondre EXACTEMENT à l'ordre de la transaction de référence ---
        let accounts = vec![
            // #1 Pool
            AccountMeta::new(self.address, false),
            // #2 User
            AccountMeta::new(*user_owner, true),
            // #3 Global Config
            AccountMeta::new_readonly(global_config_address, false),
            // #4 Base Mint
            AccountMeta::new_readonly(self.mint_a, false),
            // #5 Quote Mint
            AccountMeta::new_readonly(self.mint_b, false),
            // #6 User Base Token Account (Source pour SELL, Dest pour BUY)
            AccountMeta::new(if is_buy { user_destination_ata } else { user_source_ata }, false),
            // #7 User Quote Token Account (Source pour BUY, Dest pour SELL)
            AccountMeta::new(if is_buy { user_source_ata } else { user_destination_ata }, false),
            // #8 Pool Base Token Account
            AccountMeta::new(self.vault_a, false),
            // #9 Pool Quote Token Account
            AccountMeta::new(self.vault_b, false),
            // #10 Protocol Fee Recipient
            AccountMeta::new_readonly(*protocol_fee_recipient, false),
            // #11 Protocol Fee Recipient Token Account
            AccountMeta::new(protocol_fee_recipient_token_account, false),
            // #12 Base Token Program
            AccountMeta::new_readonly(self.mint_a_program, false),
            // #13 Quote Token Program
            AccountMeta::new_readonly(self.mint_b_program, false),
            // #14 System Program
            AccountMeta::new_readonly(system_program::id(), false),
            // #15 Associated Token Program
            AccountMeta::new_readonly(spl_associated_token_account::ID, false),
            // #16 Event Authority
            AccountMeta::new_readonly(event_authority, false),
            // #17 Program
            AccountMeta::new_readonly(PUMP_PROGRAM_ID, false),
            // #18 Coin Creator Vault Ata
            AccountMeta::new(coin_creator_vault_ata, false),
            // #19 Coin Creator Vault Authority
            AccountMeta::new_readonly(coin_creator_vault_authority, false),
            // #20 Global Volume Accumulator
            AccountMeta::new(global_volume_accumulator, false),
            // #21 User Volume Accumulator
            AccountMeta::new(user_volume_accumulator, false),
        ];

        Ok(Instruction {
            program_id: PUMP_PROGRAM_ID,
            accounts,
            data: instruction_data,
        })
    }

    pub fn create_init_user_volume_accumulator_instruction(
        &self,
        user_owner: &Pubkey,
    ) -> Result<Instruction> {
        // Discriminateur pour `init_user_volume_accumulator` trouvé dans l'IDL
        let discriminator: [u8; 8] = [94, 6, 202, 115, 255, 96, 232, 183];

        let (user_volume_accumulator, _) = Pubkey::find_program_address(
            &[b"user_volume_accumulator", user_owner.as_ref()],
            &PUMP_PROGRAM_ID
        );
        let (event_authority, _) = Pubkey::find_program_address(&[b"__event_authority"], &PUMP_PROGRAM_ID);

        let accounts = vec![
            AccountMeta::new(*user_owner, true), // Payer et Signer
            AccountMeta::new_readonly(*user_owner, false), // User
            AccountMeta::new(user_volume_accumulator, false), // Le compte à créer
            AccountMeta::new_readonly(system_program::id(), false),
            AccountMeta::new_readonly(event_authority, false),
            AccountMeta::new_readonly(PUMP_PROGRAM_ID, false),
        ];

        Ok(Instruction {
            program_id: PUMP_PROGRAM_ID,
            accounts,
            data: discriminator.to_vec(),
        })
    }
}


// --- MODULE PRIVÉ POUR LES STRUCTURES ON-CHAIN ---
// Contient les miroirs exacts des comptes de la blockchain.
// Pilier d'Excellence #6 : Robuste aux problèmes de layout mémoire.
pub mod onchain_layouts {
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
    if data.get(..8) != Some(&POOL_ACCOUNT_DISCRIMINATOR) {
        bail!("Invalid discriminator. Not a pump.fun Pool account.");
    }
    let data_slice = &data[8..];
    if data_slice.len() < std::mem::size_of::<onchain_layouts::Pool>() {
        bail!("Pump.fun Pool data length mismatch.");
    }

    let pool_struct: &onchain_layouts::Pool = bytemuck::from_bytes(
        &data_slice[..std::mem::size_of::<onchain_layouts::Pool>()]
    );

    Ok(DecodedPumpAmmPool {
        address: *address,
        mint_a: pool_struct.base_mint,
        mint_b: pool_struct.quote_mint,

        // --- LA CORRECTION DÉFINITIVE EST ICI ---
        // On lit les adresses des vaults directement depuis les données du compte,
        // comme elles sont stockées on-chain.
        vault_a: pool_struct.pool_base_token_account,
        vault_b: pool_struct.pool_quote_token_account,

        // On lit aussi le créateur, qui est nécessaire pour dériver d'autres comptes.
        coin_creator: pool_struct.coin_creator,
        // --- FIN DE LA CORRECTION ---

        protocol_fee_recipients: [Pubkey::default(); 8],
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
        mint_a_program: spl_token::id(),
        mint_b_program: spl_token::id(),
    })
}



/// Remplit les informations manquantes du pool (réserves, frais) en effectuant des appels RPC.
/// Utilise `get_multiple_accounts` pour une efficacité maximale (Pilier #4).
pub async fn hydrate(pool: &mut DecodedPumpAmmPool, rpc_client: &RpcClient) -> Result<()> {
    // 1. Calculer l'adresse du compte de configuration globale (PDA).
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

    // --- DÉBUT DE LA CORRECTION ---

    // Mint A (Base)
    // On garde l'objet `Account` complet dans `mint_a_account`
    let mint_a_account = accounts_data[2].as_ref().ok_or_else(|| anyhow!("Mint A not found for pump.fun pool {}", pool.address))?;
    // On passe `.data` au décodeur de mint
    let decoded_mint_a = spl_token_decoders::mint::decode_mint(&pool.mint_a, &mint_a_account.data)?;
    pool.mint_a_decimals = decoded_mint_a.decimals;
    pool.mint_a_transfer_fee_bps = decoded_mint_a.transfer_fee_basis_points;
    // On sauvegarde le propriétaire du compte
    pool.mint_a_program = mint_a_account.owner;

    // Mint B (Quote)
    // On fait la même chose pour le mint B
    let mint_b_account = accounts_data[3].as_ref().ok_or_else(|| anyhow!("Mint B not found for pump.fun pool {}", pool.address))?;
    let decoded_mint_b = spl_token_decoders::mint::decode_mint(&pool.mint_b, &mint_b_account.data)?;
    pool.mint_b_decimals = decoded_mint_b.decimals;
    pool.mint_b_transfer_fee_bps = decoded_mint_b.transfer_fee_basis_points;
    pool.mint_b_program = mint_b_account.owner;

    // --- FIN DE LA CORRECTION ---

    // GlobalConfig pour les Frais
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
    pool.protocol_fee_recipients = config_struct.protocol_fee_recipients;

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