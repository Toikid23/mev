// src/decoders/raydium_decoders/pool
use crate::rpc::ResilientRpcClient;
use crate::decoders::pool_operations::PoolOperations; // On importe le contrat
use bytemuck::{from_bytes, Pod, Zeroable};
use super::config;
use crate::decoders::spl_token_decoders;
use serde::{Serialize, Deserialize};
use async_trait::async_trait;
use crate::decoders::pool_operations::UserSwapAccounts;
use solana_sdk::{
    instruction::{AccountMeta, Instruction}, // <-- On importe les types ici
    pubkey::Pubkey,
};
use anyhow::{bail, Result, anyhow, Context};
use crate::state::global_cache::{CacheableData, GLOBAL_CACHE};

// Discriminator pour les comptes PoolState du programme CPMM
const CPMM_POOL_STATE_DISCRIMINATOR: [u8; 8] = [247, 237, 227, 245, 215, 195, 222, 70];

// --- STRUCTURE DE SORTIE PROPRE ---
// Contient les infos décodées et utiles du PoolState CPMM.
// Notez que nous extrayons l'adresse de l'AmmConfig pour une lecture ultérieure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecodedCpmmPool {
    pub address: Pubkey,
    pub amm_config: Pubkey,
    pub observation_key: Pubkey,
    pub token_0_mint: Pubkey,
    pub token_1_mint: Pubkey,
    pub token_0_vault: Pubkey,
    pub token_1_vault: Pubkey,
    pub mint_a_transfer_fee_bps: u16,
    pub mint_b_transfer_fee_bps: u16,
    pub token_0_program: Pubkey,
    pub token_1_program: Pubkey,
    pub status: u8,
    pub mint_0_decimals: u8,
    pub mint_1_decimals: u8,
    pub trade_fee_rate: u64,
    pub creator_fee_rate: u64,
    pub enable_creator_fee: bool,
    pub creator_fee_on: u8, // <-- AJOUTEZ CE CHAMP MANQUANT
    pub reserve_a: u64,
    pub reserve_b: u64,
    pub last_swap_timestamp: i64,
}

// Dans pool, après la struct DecodedCpmmPool
impl DecodedCpmmPool {
    pub fn fee_as_percent(&self) -> f64 {
        (self.trade_fee_rate as f64 / 1_000_000.0) * 100.0
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
    pub enable_creator_fee: u8,
    pub padding1: [u8; 6],
    pub creator_fees_token_0: u64,
    pub creator_fees_token_1: u64,
    pub padding: [u64; 28],
}

/// Tente de décoder les données brutes d'un compte Raydium CPMM PoolState.
pub fn decode_pool(address: &Pubkey, data: &[u8]) -> Result<DecodedCpmmPool> {
    if data.get(..8) != Some(&CPMM_POOL_STATE_DISCRIMINATOR) {
        bail!("Invalid discriminator.");
    }
    let data_slice = &data[8..];
    if data_slice.len() < std::mem::size_of::<CpmmPoolStateData>() {
        bail!("CPMM PoolState data length mismatch.");
    }

    // On utilise `from_bytes` pour un décodage instantané et zéro-copie
    let pool_struct: &CpmmPoolStateData = from_bytes(&data_slice[..std::mem::size_of::<CpmmPoolStateData>()]);

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
        creator_fee_on: pool_struct.creator_fee_on,
        enable_creator_fee: pool_struct.enable_creator_fee == 1,
        // Les champs suivants seront remplis par `hydrate`
        mint_a_transfer_fee_bps: 0,
        mint_b_transfer_fee_bps: 0,
        mint_0_decimals: 0,
        mint_1_decimals: 0,
        trade_fee_rate: 0,
        creator_fee_rate: 0,
        reserve_a: 0,
        reserve_b: 0,
        last_swap_timestamp: 0,
    })
}

pub async fn hydrate(pool: &mut DecodedCpmmPool, rpc_client: &ResilientRpcClient) -> Result<()> {
    // --- DÉBUT DE LA NOUVELLE LOGIQUE DE CACHE ---

    // 1. Tenter de récupérer la configuration depuis le cache global.
    let cached_config = GLOBAL_CACHE.get(&pool.amm_config);

    let decoded_config = if let Some(CacheableData::RaydiumCpmmAmmConfig(config)) = cached_config {
        println!("[Cache] HIT pour Raydium CPMM Config: {}", pool.amm_config); // Décommenter pour débugger
        config
    } else {
        println!("[Cache] MISS pour Raydium CPMM Config: {}. Fetching via RPC...", pool.amm_config); // Décommenter pour débugger
        // 2. Si le cache est vide ou expiré, faire l'appel RPC.
        let config_account_data = rpc_client
            .get_account_data(&pool.amm_config)
            .await
            .context("Échec de la récupération du compte AmmConfig pour Raydium CPMM")?;

        let new_config = config::decode_config(&config_account_data)?;

        // 3. Mettre à jour le cache avec les nouvelles données.
        GLOBAL_CACHE.put(
            pool.amm_config,
            CacheableData::RaydiumCpmmAmmConfig(new_config.clone()),
        );
        new_config
    };

    // Maintenant, `decoded_config` contient la configuration, qu'elle vienne du cache ou du RPC.
    pool.trade_fee_rate = decoded_config.trade_fee_rate;
    pool.creator_fee_rate = decoded_config.creator_fee_rate;

    // --- FIN DE LA NOUVELLE LOGIQUE DE CACHE ---

    // Le reste de la fonction `hydrate` ne change pas. On ne récupère plus
    // que les comptes qui changent fréquemment (vaults et mints).
    let accounts_to_fetch = vec![
        pool.token_0_vault,
        pool.token_1_vault,
        pool.token_0_mint,
        pool.token_1_mint,
    ];
    let accounts_data = rpc_client.get_multiple_accounts(&accounts_to_fetch).await?;
    let mut accounts_iter = accounts_data.into_iter();

    // On retire la récupération et le décodage de la config qui sont maintenant gérés par le cache.
    let vault_a_data = accounts_iter.next().flatten().ok_or_else(|| anyhow!("Vault A not found"))?.data;
    let vault_b_data = accounts_iter.next().flatten().ok_or_else(|| anyhow!("Vault B not found"))?.data;
    let mint_a_data = accounts_iter.next().flatten().ok_or_else(|| anyhow!("Mint A not found"))?.data;
    let mint_b_data = accounts_iter.next().flatten().ok_or_else(|| anyhow!("Mint B not found"))?.data;

    pool.reserve_a = u64::from_le_bytes(vault_a_data[64..72].try_into()?) ;
    pool.reserve_b = u64::from_le_bytes(vault_b_data[64..72].try_into()?) ;

    let decoded_mint_a = spl_token_decoders::mint::decode_mint(&pool.token_0_mint, &mint_a_data)?;
    pool.mint_a_transfer_fee_bps = decoded_mint_a.transfer_fee_basis_points;
    pool.mint_0_decimals = decoded_mint_a.decimals;

    let decoded_mint_b = spl_token_decoders::mint::decode_mint(&pool.token_1_mint, &mint_b_data)?;
    pool.mint_b_transfer_fee_bps = decoded_mint_b.transfer_fee_basis_points;
    pool.mint_1_decimals = decoded_mint_b.decimals;

    Ok(())
}

#[async_trait]
impl PoolOperations for DecodedCpmmPool {
    fn get_mints(&self) -> (Pubkey, Pubkey) {
        (self.token_0_mint, self.token_1_mint)
    }

    fn get_vaults(&self) -> (Pubkey, Pubkey) {
        (self.token_0_vault, self.token_1_vault)
    }

    fn get_reserves(&self) -> (u64, u64) { (self.reserve_a, self.reserve_b) }
    fn address(&self) -> Pubkey { self.address }

    // --- VERSION AMÉLIORÉE DE GET_QUOTE ---
    fn get_quote(&self, token_in_mint: &Pubkey, amount_in: u64, _current_timestamp: i64) -> Result<u64> {
        let (in_mint_fee_bps, out_mint_fee_bps, in_reserve, out_reserve) = if *token_in_mint == self.token_0_mint {
            (self.mint_a_transfer_fee_bps, self.mint_b_transfer_fee_bps, self.reserve_a, self.reserve_b)
        } else {
            (self.mint_b_transfer_fee_bps, self.mint_a_transfer_fee_bps, self.reserve_b, self.reserve_a)
        };

        // Étape 1 : Frais de transfert sur l'input (inchangé)
        let fee_on_input = (amount_in as u128 * in_mint_fee_bps as u128) / 10000;
        let amount_in_after_transfer_fee = amount_in.saturating_sub(fee_on_input as u64) as u128;

        if in_reserve == 0 || out_reserve == 0 { return Ok(0); }

        const FEE_DENOMINATOR: u128 = 1_000_000;

        // Étape 2 : Calculer les frais de trading avec un arrondi au plafond (ceil), comme le programme on-chain.
        let trade_fee = (amount_in_after_transfer_fee * self.trade_fee_rate as u128).div_ceil(FEE_DENOMINATOR);

        // Étape 3 : Soustraire les frais pour obtenir le montant net pour le swap.
        let creator_fee = if self.enable_creator_fee {
            (amount_in_after_transfer_fee * self.creator_fee_rate as u128).div_ceil(FEE_DENOMINATOR)
        } else {
            0
        };
        // --- FIN DE L'AJOUT ---

        let amount_in_less_fees = amount_in_after_transfer_fee
            .saturating_sub(trade_fee)
            .saturating_sub(creator_fee); // <-- On soustrait aussi les frais du créateur

        // Étape 4 : Calculer le swap avec la formule de produit constant (arrondi au plancher).
        let numerator = amount_in_less_fees * out_reserve as u128;
        let denominator = in_reserve as u128 + amount_in_less_fees;
        if denominator == 0 { return Ok(0); }
        let gross_amount_out = (numerator / denominator) as u64;

        // Étape 5 : Frais de transfert sur l'output (inchangé)
        let fee_on_output = (gross_amount_out as u128 * out_mint_fee_bps as u128) / 10000;
        let final_amount_out = gross_amount_out.saturating_sub(fee_on_output as u64);

        Ok(final_amount_out)
    }

    // --- NOUVELLE FONCTION get_required_input ---
    fn get_required_input(
        &mut self,
        token_out_mint: &Pubkey,
        amount_out: u64,
        _current_timestamp: i64,
    ) -> Result<u64> {
        if amount_out == 0 { return Ok(0); }

        let (in_mint_fee_bps, out_mint_fee_bps, in_reserve, out_reserve) = if *token_out_mint == self.token_1_mint {
            (self.mint_a_transfer_fee_bps, self.mint_b_transfer_fee_bps, self.reserve_a, self.reserve_b)
        } else {
            (self.mint_b_transfer_fee_bps, self.mint_a_transfer_fee_bps, self.reserve_b, self.reserve_a)
        };

        if out_reserve == 0 || in_reserve == 0 { return Err(anyhow!("Pool has no liquidity")); }

        const BPS_DENOMINATOR: u128 = 10000;
        const FEE_DENOMINATOR: u128 = 1_000_000;

        // Étape 1: Inverser les frais de transfert Token-2022 sur la SORTIE (inchangé)
        let gross_amount_out = if out_mint_fee_bps > 0 {
            let numerator = (amount_out as u128).saturating_mul(BPS_DENOMINATOR);
            let denominator = BPS_DENOMINATOR.saturating_sub(out_mint_fee_bps as u128);
            numerator.div_ceil(denominator)
        } else {
            amount_out as u128
        };
        if gross_amount_out >= out_reserve as u128 { return Err(anyhow!("Amount out is too high")); }

        // Étape 2: Inverser la formule du produit constant (inchangé)
        let numerator = gross_amount_out.saturating_mul(in_reserve as u128);
        let denominator = (out_reserve as u128).saturating_sub(gross_amount_out);
        let amount_in_less_fees = numerator.div_ceil(denominator);

        // Étape 3: Inverser les frais de trading du pool. C'est le miroir de `get_quote`.
        let total_fee_rate = if self.enable_creator_fee {
            self.trade_fee_rate + self.creator_fee_rate
        } else {
            self.trade_fee_rate
        };

        let amount_in_after_transfer_fee = if total_fee_rate > 0 {
            let num = amount_in_less_fees.saturating_mul(FEE_DENOMINATOR);
            let den = FEE_DENOMINATOR.saturating_sub(total_fee_rate as u128);
            num.div_ceil(den)
        } else {
            amount_in_less_fees
        };

        // Étape 4: Inverser les frais de transfert Token-2022 sur l'ENTRÉE (inchangé)
        let required_amount_in = if in_mint_fee_bps > 0 {
            let num = amount_in_after_transfer_fee.saturating_mul(BPS_DENOMINATOR);
            let den = BPS_DENOMINATOR.saturating_sub(in_mint_fee_bps as u128);
            num.div_ceil(den)
        } else {
            amount_in_after_transfer_fee
        };

        Ok(required_amount_in as u64)
    }

    fn create_swap_instruction(
        &self,
        token_in_mint: &Pubkey,
        amount_in: u64,
        min_amount_out: u64,
        user_accounts: &UserSwapAccounts,
    ) -> Result<Instruction> {
        let instruction_discriminator: [u8; 8] = [143, 190, 90, 218, 196, 30, 51, 222];

        let mut instruction_data = Vec::new();
        instruction_data.extend_from_slice(&instruction_discriminator);
        instruction_data.extend_from_slice(&amount_in.to_le_bytes());
        instruction_data.extend_from_slice(&min_amount_out.to_le_bytes());

        let input_is_token_0 = *token_in_mint == self.token_0_mint;
        let (input_vault, output_vault, input_mint, output_mint, input_token_program, output_token_program) = if input_is_token_0 {
            (self.token_0_vault, self.token_1_vault, self.token_0_mint, self.token_1_mint, self.token_0_program, self.token_1_program)
        } else {
            (self.token_1_vault, self.token_0_vault, self.token_1_mint, self.token_0_mint, self.token_1_program, self.token_0_program)
        };

        let (authority, _) = Pubkey::find_program_address(&[b"vault_and_lp_mint_auth_seed"], &solana_sdk::pubkey!("CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C"));

        let accounts = vec![
            AccountMeta::new_readonly(user_accounts.owner, true),
            AccountMeta::new_readonly(authority, false),
            AccountMeta::new_readonly(self.amm_config, false),
            AccountMeta::new(self.address, false),
            AccountMeta::new(user_accounts.source, false),
            AccountMeta::new(user_accounts.destination, false),
            AccountMeta::new(input_vault, false),
            AccountMeta::new(output_vault, false),
            AccountMeta::new_readonly(input_token_program, false),
            AccountMeta::new_readonly(output_token_program, false),
            AccountMeta::new_readonly(input_mint, false),
            AccountMeta::new_readonly(output_mint, false),
            AccountMeta::new(self.observation_key, false),
        ];

        Ok(Instruction {
            program_id: solana_sdk::pubkey!("CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C"),
            accounts,
            data: instruction_data,
        })
    }
}