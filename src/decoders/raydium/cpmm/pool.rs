// src/decoders/raydium_decoders/pool

use crate::decoders::pool_operations::PoolOperations; // On importe le contrat
use bytemuck::{from_bytes, Pod, Zeroable};
use solana_sdk::pubkey::Pubkey;
use anyhow::{bail, Result, anyhow};
use solana_client::nonblocking::rpc_client::RpcClient;
use super::config;
use crate::decoders::spl_token_decoders;
use num_integer::Integer;
use serde::{Serialize, Deserialize};
use async_trait::async_trait;

// Discriminator pour les comptes PoolState du programme CPMM
const CPMM_POOL_STATE_DISCRIMINATOR: [u8; 8] = [247, 237, 227, 245, 215, 195, 222, 70];

// --- STRUCTURE DE SORTIE PROPRE ---
// Contient les infos décodées et utiles du PoolState CPMM.
// Notez que nous extrayons l'adresse de l'AmmConfig pour une lecture ultérieure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecodedCpmmPool {
    pub address: Pubkey,
    pub amm_config: Pubkey, // Pour aller chercher les frais plus tard
    pub observation_key: Pubkey,
    pub token_0_mint: Pubkey,
    pub token_1_mint: Pubkey,
    pub token_0_vault: Pubkey,
    pub token_1_vault: Pubkey,
    pub mint_a_transfer_fee_bps: u16,
    pub mint_b_transfer_fee_bps: u16,
    pub token_0_program: Pubkey, // <--- AJOUTER
    pub token_1_program: Pubkey,
    pub status: u8,
    pub mint_0_decimals: u8,
    pub mint_1_decimals: u8,
    // Les champs "intelligents"
    pub trade_fee_rate: u64,
    pub reserve_a: u64,
    pub reserve_b: u64,
    pub last_swap_timestamp: i64,
}

// Dans pool, après la struct DecodedCpmmPool
impl DecodedCpmmPool {
    pub fn fee_as_percent(&self) -> f64 {
        (self.trade_fee_rate as f64 / 1_000_000.0) * 100.0
    }

    // dans src/decoders/raydium/cpmm/pool.rs

    pub fn create_swap_instruction(
        &self,
        user_source_token_account: &Pubkey,
        user_destination_token_account: &Pubkey,
        user_owner: &Pubkey,
        input_is_token_0: bool,
        amount_in: u64,
        minimum_amount_out: u64,
    ) -> Result<solana_sdk::instruction::Instruction> {
        let instruction_discriminator: [u8; 8] = [143, 190, 90, 218, 196, 30, 51, 222];

        let mut instruction_data = Vec::new();
        instruction_data.extend_from_slice(&instruction_discriminator);
        instruction_data.extend_from_slice(&amount_in.to_le_bytes());
        instruction_data.extend_from_slice(&minimum_amount_out.to_le_bytes());

        let (input_vault, output_vault, input_mint, output_mint, input_token_program, output_token_program) = if input_is_token_0 {
            (self.token_0_vault, self.token_1_vault, self.token_0_mint, self.token_1_mint, self.token_0_program, self.token_1_program)
        } else {
            (self.token_1_vault, self.token_0_vault, self.token_1_mint, self.token_0_mint, self.token_1_program, self.token_0_program)
        };

        let (authority, _) = Pubkey::find_program_address(&[b"vault_and_lp_mint_auth_seed"], &solana_sdk::pubkey!("CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C"));

        // --- LISTE DES COMPTES DE LA VERSION ORIGINALE QUI FONCTIONNAIT ---
        let accounts = vec![
            solana_sdk::instruction::AccountMeta::new_readonly(*user_owner, true),
            solana_sdk::instruction::AccountMeta::new_readonly(authority, false),
            solana_sdk::instruction::AccountMeta::new_readonly(self.amm_config, false),
            solana_sdk::instruction::AccountMeta::new(self.address, false),
            solana_sdk::instruction::AccountMeta::new(*user_source_token_account, false),
            solana_sdk::instruction::AccountMeta::new(*user_destination_token_account, false),
            solana_sdk::instruction::AccountMeta::new(input_vault, false),
            solana_sdk::instruction::AccountMeta::new(output_vault, false),
            solana_sdk::instruction::AccountMeta::new_readonly(input_token_program, false),
            solana_sdk::instruction::AccountMeta::new_readonly(output_token_program, false),
            solana_sdk::instruction::AccountMeta::new_readonly(input_mint, false),
            solana_sdk::instruction::AccountMeta::new_readonly(output_mint, false),
            solana_sdk::instruction::AccountMeta::new(self.observation_key, false),
        ];

        Ok(solana_sdk::instruction::Instruction {
            program_id: solana_sdk::pubkey!("CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C"),
            accounts,
            data: instruction_data,
        })
    }
}

// --- STRUCTURE DE DONNÉES BRUTES (Miroir exact de l'IDL) ---
#[repr(C, packed)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
struct CpmmPoolStateData {
    pub amm_config: Pubkey,
    pub pool_creator: Pubkey,
    pub token_0_vault: Pubkey,
    pub token_1_vault: Pubkey,
    pub lp_mint: Pubkey,
    pub token_0_mint: Pubkey,
    pub token_1_mint: Pubkey,
    pub token_0_program: Pubkey,
    pub token_1_program: Pubkey,
    pub observation_key: Pubkey,
    pub auth_bump: u8,
    pub status: u8,
    pub lp_mint_decimals: u8,
    pub mint_0_decimals: u8,
    pub mint_1_decimals: u8,
    pub lp_supply: u64,
    pub protocol_fees_token_0: u64,
    pub protocol_fees_token_1: u64,
    pub fund_fees_token_0: u64,
    pub fund_fees_token_1: u64,
    pub open_time: u64,
    pub recent_epoch: u64,
    pub creator_fee_on: u8,
    pub enable_creator_fee: u8, // bool est 1 byte
    pub padding1: [u8; 6],
    pub creator_fees_token_0: u64,
    pub creator_fees_token_1: u64,
    // Padding final réduit
    pub padding: [u64; 28],
}

/// Tente de décoder les données brutes d'un compte Raydium CPMM PoolState.
pub fn decode_pool(address: &Pubkey, data: &[u8]) -> Result<DecodedCpmmPool> {
    // Étape 1: Vérifier le discriminateur. C'est le seul moyen fiable d'identifier un PoolState.
    if data.get(..8) != Some(&CPMM_POOL_STATE_DISCRIMINATOR) {
        bail!("Invalid discriminator. Not a Raydium CPMM PoolState account.");
    }

    let data_slice = &data[8..];

    // Étape 2: Vérifier que les données sont AU MOINS assez longues.
    // Cela nous protège contre les données corrompues et permet les futures mises à jour du programme.
    if data_slice.len() < std::mem::size_of::<CpmmPoolStateData>() {
        bail!(
            "CPMM PoolState data length mismatch. Expected at least {}, got {}.",
            std::mem::size_of::<CpmmPoolStateData>(),
            data_slice.len()
        );
    }

    // Étape 3: "Caster" les données en utilisant la taille de notre struct.
    // On ignore les octets supplémentaires s'il y en a.
    let pool_struct: &CpmmPoolStateData = from_bytes(&data_slice[..std::mem::size_of::<CpmmPoolStateData>()]);

    // Étape 4: Créer la sortie propre et unifiée
    Ok(DecodedCpmmPool {
        address: *address,
        amm_config: pool_struct.amm_config,
        observation_key: pool_struct.observation_key,
        token_0_mint: pool_struct.token_0_mint,
        token_1_mint: pool_struct.token_1_mint,
        token_0_vault: pool_struct.token_0_vault,
        token_1_vault: pool_struct.token_1_vault,
        token_0_program: pool_struct.token_0_program,
        token_1_program: pool_struct.token_1_program,
        status: pool_struct.status,
        mint_a_transfer_fee_bps: 0,
        mint_b_transfer_fee_bps: 0,
        mint_0_decimals: 0,
        mint_1_decimals: 0,
        trade_fee_rate: 0,
        reserve_a: 0,
        reserve_b: 0,
        last_swap_timestamp: 0,
    })
}

#[async_trait]
impl PoolOperations for DecodedCpmmPool {
    fn get_mints(&self) -> (Pubkey, Pubkey) {
        (self.token_0_mint, self.token_1_mint)
    }

    fn get_vaults(&self) -> (Pubkey, Pubkey) {
        (self.token_0_vault, self.token_1_vault)
    }
    fn address(&self) -> Pubkey { self.address }

    // --- VERSION AMÉLIORÉE DE GET_QUOTE ---
    fn get_quote(&self, token_in_mint: &Pubkey, amount_in: u64, _current_timestamp: i64) -> Result<u64> {
        let (in_mint_fee_bps, out_mint_fee_bps, in_reserve, out_reserve) = if *token_in_mint == self.token_0_mint {
            (self.mint_a_transfer_fee_bps, self.mint_b_transfer_fee_bps, self.reserve_a, self.reserve_b)
        } else if *token_in_mint == self.token_1_mint {
            (self.mint_b_transfer_fee_bps, self.mint_a_transfer_fee_bps, self.reserve_b, self.reserve_a)
        } else {
            return Err(anyhow!("Input token does not belong to this pool."));
        };

        // Étape 1 : Frais de transfert Token-2022 (inchangé)
        let fee_on_input = (amount_in as u128 * in_mint_fee_bps as u128) / 10000;
        let amount_in_after_transfer_fee = amount_in.saturating_sub(fee_on_input as u64) as u128;

        if in_reserve == 0 || out_reserve == 0 {
            return Ok(0);
        }

        // --- DÉBUT DE LA LOGIQUE DE FRAIS 1:1 ---
        const FEE_DENOMINATOR: u128 = 1_000_000;

        // Étape 2 : Calculer les frais de trading avec un arrondi au plafond (ceil).
        // C'est la réplique exacte du code on-chain.
        let trade_fee = (amount_in_after_transfer_fee * self.trade_fee_rate as u128).div_ceil(FEE_DENOMINATOR);

        // Étape 3 : Soustraire les frais pour obtenir le montant net pour le swap.
        let amount_in_less_fees = amount_in_after_transfer_fee.saturating_sub(trade_fee);

        // Étape 4 : Calculer le swap avec la formule de produit constant pure (inchangé).
        let numerator = amount_in_less_fees * out_reserve as u128;
        let denominator = in_reserve as u128 + amount_in_less_fees;
        if denominator == 0 { return Ok(0); }
        let gross_amount_out = (numerator / denominator) as u64;

        // --- FIN DE LA LOGIQUE DE FRAIS 1:1 ---

        // Étape 5 : Frais de transfert sur l'output (inchangé)
        let fee_on_output = (gross_amount_out as u128 * out_mint_fee_bps as u128) / 10000;
        let final_amount_out = gross_amount_out.saturating_sub(fee_on_output as u64);

        Ok(final_amount_out)
    }
    async fn get_quote_async(&mut self, token_in_mint: &Pubkey, amount_in: u64, _rpc_client: &RpcClient) -> Result<u64> {
        self.get_quote(token_in_mint, amount_in, 0)
    }
}
pub async fn hydrate(pool: &mut DecodedCpmmPool, rpc_client: &RpcClient) -> Result<()> {
    // ... (votre `tokio::join!` existant est parfait et reste inchangé) ...
    let (config_res, vault_a_res, vault_b_res, mint_a_res, mint_b_res) = tokio::join!(
        rpc_client.get_account_data(&pool.amm_config),
        rpc_client.get_account_data(&pool.token_0_vault),
        rpc_client.get_account_data(&pool.token_1_vault),
        rpc_client.get_account_data(&pool.token_0_mint),
        rpc_client.get_account_data(&pool.token_1_mint)
    );

    // ... (la logique pour `config_data`, `vault_a_data`, `vault_b_data` est inchangée) ...
    let config_data = config_res?;
    let decoded_config = config::decode_config(&config_data)?;
    pool.trade_fee_rate = decoded_config.trade_fee_rate;

    let vault_a_data = vault_a_res?;
    pool.reserve_a = u64::from_le_bytes(vault_a_data[64..72].try_into()?);
    let vault_b_data = vault_b_res?;
    pool.reserve_b = u64::from_le_bytes(vault_b_data[64..72].try_into()?);

    // --- MISE À JOUR DE LA LOGIQUE D'HYDRATATION DES MINTS ---
    let mint_a_data = mint_a_res?;
    let decoded_mint_a = spl_token_decoders::mint::decode_mint(&pool.token_0_mint, &mint_a_data)?;
    pool.mint_a_transfer_fee_bps = decoded_mint_a.transfer_fee_basis_points;
    // AJOUTEZ CETTE LIGNE :
    pool.mint_0_decimals = decoded_mint_a.decimals;

    let mint_b_data = mint_b_res?;
    let decoded_mint_b = spl_token_decoders::mint::decode_mint(&pool.token_1_mint, &mint_b_data)?;
    pool.mint_b_transfer_fee_bps = decoded_mint_b.transfer_fee_basis_points;
    // AJOUTEZ CETTE LIGNE :
    pool.mint_1_decimals = decoded_mint_b.decimals;
    // --- FIN DE LA MISE À JOUR ---

    Ok(())
}