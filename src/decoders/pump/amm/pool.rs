use bytemuck::{Pod, Zeroable};
use solana_sdk::pubkey::Pubkey;
use anyhow::{anyhow, bail, Result};
use crate::decoders::spl_token_decoders;
use solana_sdk::instruction::{Instruction, AccountMeta};
use solana_sdk_ids::system_program;
use spl_associated_token_account::get_associated_token_address_with_program_id;
use serde::{Serialize, Deserialize};
use async_trait::async_trait;
use crate::decoders::pool_operations::{PoolOperations, UserSwapAccounts};
use std::str::FromStr;
use borsh::{BorshDeserialize, BorshSerialize};
use crate::decoders::spl_token_decoders::mint::DecodedMint;
use crate::rpc::ResilientRpcClient;
use crate::state::global_cache::{CacheableData, GLOBAL_CACHE};
use anyhow::Context;

// --- CONSTANTES ---
pub const PUMP_PROGRAM_ID: Pubkey = solana_sdk::pubkey!("pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA");
pub const PUMP_FEE_PROGRAM_ID: Pubkey = solana_sdk::pubkey!("pfeeUxB6jkeY1Hxd7CsFCAjcbHA9rWtchMGdZ6VojVZ");
const POOL_ACCOUNT_DISCRIMINATOR: [u8; 8] = [241, 154, 109, 4, 17, 177, 109, 188];
const GLOBAL_CONFIG_ACCOUNT_DISCRIMINATOR: [u8; 8] = [149, 8, 156, 202, 160, 252, 176, 217];
const FEE_CONFIG_DISCRIMINATOR: [u8; 8] = [143, 52, 146, 187, 219, 123, 76, 155];


#[derive(Debug, Clone, Serialize, Deserialize, Default, BorshDeserialize, BorshSerialize)]
pub struct DecodedFees {
    pub lp_fee_bps: u64,
    pub protocol_fee_bps: u64,
    pub creator_fee_bps: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, BorshDeserialize)]
pub struct DecodedFeeTier {
    pub market_cap_lamports_threshold: u128,
    pub fees: DecodedFees,
}

#[derive(Debug, Clone, Serialize, Deserialize, BorshDeserialize)]
pub struct DecodedFeeConfig {
    pub bump: u8,
    pub admin: Pubkey,
    pub flat_fees: DecodedFees,
    pub fee_tiers: Vec<DecodedFeeTier>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecodedPumpAmmPool {
    pub address: Pubkey,
    pub creator: Pubkey,
    pub mint_a: Pubkey,
    pub mint_b: Pubkey,
    pub vault_a: Pubkey,
    pub vault_b: Pubkey,
    pub coin_creator: Pubkey,
    pub protocol_fee_recipients: [Pubkey; 8],
    pub reserve_a: u64,
    pub reserve_b: u64,
    pub mint_a_decoded: DecodedMint,
    pub mint_b_decoded: DecodedMint,
    pub global_config: onchain_layouts::GlobalConfig,
    pub fee_config: Option<DecodedFeeConfig>,
    pub mint_a_program: Pubkey,
    pub mint_b_program: Pubkey,
    pub last_swap_timestamp: i64,
}

pub mod onchain_layouts {
    use super::*;
    #[repr(C, packed)]
    #[derive(Clone, Copy, Pod, Zeroable, Debug, Serialize, Deserialize)]
    pub struct Pool {
        pub pool_bump: u8, pub index: u16, pub creator: Pubkey,
        pub base_mint: Pubkey, pub quote_mint: Pubkey, pub lp_mint: Pubkey,
        pub pool_base_token_account: Pubkey, pub pool_quote_token_account: Pubkey,
        pub lp_supply: u64, pub coin_creator: Pubkey,
    }
    #[repr(C, packed)]
    #[derive(Clone, Copy, Pod, Zeroable, Debug, Serialize, Deserialize, Default)] // <-- AJOUT DE DEFAULT
    pub struct GlobalConfig {
        pub admin: Pubkey, pub lp_fee_basis_points: u64, pub protocol_fee_basis_points: u64,
        pub disable_flags: u8, pub protocol_fee_recipients: [Pubkey; 8],
        pub coin_creator_fee_basis_points: u64, pub admin_set_coin_creator_authority: Pubkey,
    }
}

pub fn decode_pool(address: &Pubkey, data: &[u8]) -> Result<DecodedPumpAmmPool> {
    if data.get(..8) != Some(&POOL_ACCOUNT_DISCRIMINATOR) { bail!("Invalid discriminator."); }
    let data_slice = &data[8..];
    if data_slice.len() < size_of::<onchain_layouts::Pool>() { bail!("Data length mismatch."); }
    let pool_struct: &onchain_layouts::Pool = bytemuck::from_bytes(&data_slice[..size_of::<onchain_layouts::Pool>()]);

    Ok(DecodedPumpAmmPool {
        address: *address, creator: pool_struct.creator, mint_a: pool_struct.base_mint,
        mint_b: pool_struct.quote_mint, vault_a: pool_struct.pool_base_token_account,
        vault_b: pool_struct.pool_quote_token_account, coin_creator: pool_struct.coin_creator,
        protocol_fee_recipients: [Pubkey::default(); 8], reserve_a: 0, reserve_b: 0,
        mint_a_decoded: DecodedMint { address: Pubkey::default(), supply: 0, decimals: 0, transfer_fee_basis_points: 0, max_transfer_fee: 0 },
        mint_b_decoded: DecodedMint { address: Pubkey::default(), supply: 0, decimals: 0, transfer_fee_basis_points: 0, max_transfer_fee: 0 },
        global_config: onchain_layouts::GlobalConfig::default(), // <-- UTILISATION DE LA MÉTHODE SAFE
        fee_config: None,
        mint_a_program: spl_token::id(), mint_b_program: spl_token::id(),
        last_swap_timestamp: 0,
    })
}

pub async fn hydrate(pool: &mut DecodedPumpAmmPool, rpc_client: &ResilientRpcClient) -> Result<()> {
    let (global_config_address, _) = Pubkey::find_program_address(&[b"global_config"], &PUMP_PROGRAM_ID);
    let (fee_config_address, _) = Pubkey::find_program_address(&[b"fee_config", PUMP_PROGRAM_ID.as_ref()], &PUMP_FEE_PROGRAM_ID);

    // --- DÉBUT DE LA NOUVELLE LOGIQUE DE CACHE POUR GLOBAL_CONFIG ---

    // 1. Tenter de récupérer GlobalConfig depuis le cache.
    let cached_config = GLOBAL_CACHE.get(&global_config_address);

    let global_config = if let Some(CacheableData::PumpAmmGlobalConfig(config)) = cached_config {
        println!("[Cache] HIT pour Pump.fun GlobalConfig: {}", global_config_address);
        config
    } else {
        println!("[Cache] MISS pour Pump.fun GlobalConfig: {}. Fetching via RPC...", global_config_address);
        // 2. Cache MISS: Faire l'appel RPC.
        let global_config_data = rpc_client
            .get_account_data(&global_config_address)
            .await
            .context("Échec de la récupération du GlobalConfig de Pump.fun")?;

        if global_config_data.get(..8) != Some(&GLOBAL_CONFIG_ACCOUNT_DISCRIMINATOR) {
            bail!("Invalid GlobalConfig discriminator");
        }
        let config_data_slice = &global_config_data[8..];
        let new_config: onchain_layouts::GlobalConfig = *bytemuck::from_bytes(
            &config_data_slice[..size_of::<onchain_layouts::GlobalConfig>()],
        );

        // 3. Mettre à jour le cache.
        GLOBAL_CACHE.put(
            global_config_address,
            CacheableData::PumpAmmGlobalConfig(new_config), // Pas besoin de .clone() car GlobalConfig est Copy
        );
        new_config
    };

    // Assigner la config (du cache ou du RPC) au pool.
    pool.global_config = global_config;
    pool.protocol_fee_recipients = pool.global_config.protocol_fee_recipients;

    // --- FIN DE LA LOGIQUE DE CACHE ---

    // Le reste de la fonction ne récupère plus que les comptes variables.
    let accounts_to_fetch = vec![
        pool.vault_a, pool.vault_b, pool.mint_a, pool.mint_b,
        fee_config_address // On garde fee_config car il pourrait être plus dynamique
    ];
    let mut accounts_data = rpc_client.get_multiple_accounts(&accounts_to_fetch).await?;

    let vault_a_data = accounts_data[0].take().ok_or_else(|| anyhow!("Vault A not found"))?.data;
    pool.reserve_a = u64::from_le_bytes(vault_a_data[64..72].try_into()?);

    let vault_b_data = accounts_data[1].take().ok_or_else(|| anyhow!("Vault B not found"))?.data;
    pool.reserve_b = u64::from_le_bytes(vault_b_data[64..72].try_into()?);

    let mint_a_account = accounts_data[2].take().ok_or_else(|| anyhow!("Mint A not found"))?;
    pool.mint_a_decoded = spl_token_decoders::mint::decode_mint(&pool.mint_a, &mint_a_account.data)?;
    pool.mint_a_program = mint_a_account.owner;

    let mint_b_account = accounts_data[3].take().ok_or_else(|| anyhow!("Mint B not found"))?;
    pool.mint_b_decoded = spl_token_decoders::mint::decode_mint(&pool.mint_b, &mint_b_account.data)?;
    pool.mint_b_program = mint_b_account.owner;

    // La logique pour FeeConfig reste inchangée, car il pourrait être mis à jour plus souvent.
    if let Some(fee_config_account) = accounts_data[4].take() {
        if fee_config_account.data.get(..8) == Some(&FEE_CONFIG_DISCRIMINATOR) {
            let mut data_slice = &fee_config_account.data[8..];
            pool.fee_config = Some(<DecodedFeeConfig as BorshDeserialize>::deserialize(&mut data_slice)?);
        }
    }
    Ok(())
}

fn pool_market_cap(base_mint_supply: u64, base_reserve: u64, quote_reserve: u64) -> Result<u128> {
    if base_reserve == 0 { return Ok(0); }
    Ok((quote_reserve as u128).saturating_mul(base_mint_supply as u128) / (base_reserve as u128))
}

fn calculate_fee_tier(fee_tiers: &[DecodedFeeTier], market_cap: u128) -> DecodedFees {
    if fee_tiers.is_empty() { return DecodedFees::default(); }
    let first_tier = &fee_tiers[0];
    if market_cap < first_tier.market_cap_lamports_threshold {
        return first_tier.fees.clone();
    }
    for tier in fee_tiers.iter().rev() {
        if market_cap >= tier.market_cap_lamports_threshold {
            return tier.fees.clone();
        }
    }
    first_tier.fees.clone()
}

fn is_pump_pool(base_mint: &Pubkey, pool_creator: &Pubkey) -> bool {
    let (pump_pool_authority, _) = Pubkey::find_program_address(&[b"pool-authority", base_mint.as_ref()], &Pubkey::from_str("6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P").unwrap());
    *pool_creator == pump_pool_authority
}

// CORRECTION : La fonction n'est plus `async` et n'a plus besoin du `rpc_client`.
fn compute_fees_bps(pool: &DecodedPumpAmmPool, base_mint_supply: u64, _trade_size: u64) -> Result<DecodedFees> {
    if let Some(fee_config) = &pool.fee_config {
        return if is_pump_pool(&pool.mint_a, &pool.creator) {
            let market_cap = pool_market_cap(base_mint_supply, pool.reserve_a, pool.reserve_b)?;
            Ok(calculate_fee_tier(&fee_config.fee_tiers, market_cap))
        } else {
            Ok(fee_config.flat_fees.clone())
        }
    }
    Ok(DecodedFees {
        lp_fee_bps: pool.global_config.lp_fee_basis_points,
        protocol_fee_bps: pool.global_config.protocol_fee_basis_points,
        creator_fee_bps: pool.global_config.coin_creator_fee_basis_points,
    })
}

#[async_trait]
impl PoolOperations for DecodedPumpAmmPool {
    fn get_mints(&self) -> (Pubkey, Pubkey) { (self.mint_a, self.mint_b) }
    fn get_vaults(&self) -> (Pubkey, Pubkey) { (self.vault_a, self.vault_b) }
    fn get_reserves(&self) -> (u64, u64) {
        (self.reserve_a, self.reserve_b)
    }
    fn address(&self) -> Pubkey { self.address }

    fn get_quote(&self, token_in_mint: &Pubkey, amount_in: u64, _current_timestamp: i64) -> Result<u64> {
        let is_buy = *token_in_mint == self.mint_b;
        let fees = compute_fees_bps(self, self.mint_a_decoded.supply, amount_in)?;

        if is_buy {
            let amount_in_u128 = amount_in as u128;
            let total_fee_bps = fees.lp_fee_bps + fees.protocol_fee_bps + fees.creator_fee_bps;
            let denominator = 10_000u128.saturating_add(total_fee_bps as u128);
            let net_amount_in = amount_in_u128.saturating_mul(10_000).saturating_div(denominator);
            let numerator = (self.reserve_a as u128).saturating_mul(net_amount_in);
            let effective_denominator = (self.reserve_b as u128).saturating_add(net_amount_in);
            Ok((numerator / effective_denominator) as u64)
        } else {
            let numerator = (amount_in as u128) * (self.reserve_b as u128);
            let denominator = (self.reserve_a as u128) + (amount_in as u128);
            if denominator == 0 { return Ok(0); }
            let gross_amount_out = numerator / denominator;
            let total_fee_bps = fees.lp_fee_bps + fees.protocol_fee_bps + fees.creator_fee_bps;
            let total_fee = (gross_amount_out * total_fee_bps as u128) / 10_000;
            Ok(gross_amount_out.saturating_sub(total_fee) as u64)
        }
    }

    fn get_required_input(&mut self, token_out_mint: &Pubkey, amount_out: u64, _current_timestamp: i64) -> Result<u64> {
        if amount_out == 0 { return Ok(0); }
        let is_buy = *token_out_mint == self.mint_a;
        if !is_buy { return Err(anyhow!("get_required_input pour pump.fun est optimisé pour les achats.")); }

        let in_reserve = self.reserve_b;
        let out_reserve = self.reserve_a;

        if out_reserve <= amount_out { return Err(anyhow!("Amount out exceeds pool reserves.")); }

        // 1. Calculer le coût NET que le pool doit recevoir (formule AMM inversée).
        // Cette partie est la base et ne change pas.
        let gross_amount_out = amount_out as u128;
        let numerator = (in_reserve as u128).saturating_mul(gross_amount_out);
        let denominator = (out_reserve as u128).saturating_sub(gross_amount_out);
        let net_amount_in = numerator.div_ceil(denominator);

        // 2. Obtenir les taux de frais BPS.
        // L'estimation du `trade_size` avec `net_amount_in` est la meilleure approximation possible à ce stade.
        let fees = compute_fees_bps(self, self.mint_a_decoded.supply, net_amount_in as u64)?;

        // 3. Calculer le coût total en une seule passe mathématique.
        // TotalCost = NetCost * ( (10000 + LP_Bps + Prot_Bps + Creat_Bps) / 10000 )
        const BPS_DENOMINATOR: u128 = 10_000;

        let creator_fee_bps = if self.coin_creator != Pubkey::default() {
            fees.creator_fee_bps
        } else {
            0
        };

        let total_fee_bps = fees.lp_fee_bps + fees.protocol_fee_bps + creator_fee_bps;

        let final_total_cost = net_amount_in
            .saturating_mul(BPS_DENOMINATOR + total_fee_bps as u128)
            .div_ceil(BPS_DENOMINATOR);

        // Les logs de débogage restent utiles pour valider
        println!("\n--- [get_required_input DEBUG - OPTIMIZED] ---");
        println!("  - Coût NET (calculé)   : {}", net_amount_in);
        println!("  - Frais sélectionnés (LP, Prot, Créateur) bps: ({}, {}, {})", fees.lp_fee_bps, fees.protocol_fee_bps, fees.creator_fee_bps);
        println!("  - Multiplicateur de coût total (bps): {}", 10000 + total_fee_bps);
        println!("  - Coût Total PRÉDIT    : {}", final_total_cost);
        println!("------------------------------------");

        Ok(final_total_cost as u64)
    }

    fn create_swap_instruction(&self, token_in_mint: &Pubkey, amount_in: u64, min_amount_out: u64, user_accounts: &UserSwapAccounts) -> Result<Instruction> {
        let is_buy = *token_in_mint == self.mint_b;
        let mut instruction_data = Vec::new();
        if is_buy {
            let discriminator: [u8; 8] = [102, 6, 61, 18, 1, 218, 235, 234];
            instruction_data.extend_from_slice(&discriminator);
            instruction_data.extend_from_slice(&min_amount_out.to_le_bytes());
            instruction_data.extend_from_slice(&amount_in.to_le_bytes());
            instruction_data.push(1); instruction_data.push(1);
        } else {
            let discriminator: [u8; 8] = [51, 230, 133, 164, 1, 127, 131, 173];
            instruction_data.extend_from_slice(&discriminator);
            instruction_data.extend_from_slice(&amount_in.to_le_bytes());
            instruction_data.extend_from_slice(&min_amount_out.to_le_bytes());
        };
        let (user_base_token_account, user_quote_token_account) = if is_buy {
            (user_accounts.destination, user_accounts.source)
        } else {
            (user_accounts.source, user_accounts.destination)
        };
        let (global_config_address, _) = Pubkey::find_program_address(&[b"global_config"], &PUMP_PROGRAM_ID);
        let (fee_config, _) = Pubkey::find_program_address(&[b"fee_config", PUMP_PROGRAM_ID.as_ref()], &PUMP_FEE_PROGRAM_ID);
        let protocol_fee_recipient = self.protocol_fee_recipients.iter().find(|&&key| key != Pubkey::default()).unwrap_or(&self.protocol_fee_recipients[0]);
        let protocol_fee_recipient_token_account = get_associated_token_address_with_program_id(protocol_fee_recipient, &self.mint_b, &self.mint_b_program);
        let (coin_creator_vault_authority, _) = Pubkey::find_program_address(&[b"creator_vault", self.coin_creator.as_ref()], &PUMP_PROGRAM_ID);
        let coin_creator_vault_ata = get_associated_token_address_with_program_id(&coin_creator_vault_authority, &self.mint_b, &self.mint_b_program);
        let (user_volume_accumulator, _) = Pubkey::find_program_address(&[b"user_volume_accumulator", user_accounts.owner.as_ref()], &PUMP_PROGRAM_ID);
        let (global_volume_accumulator, _) = Pubkey::find_program_address(&[b"global_volume_accumulator"], &PUMP_PROGRAM_ID);
        let (event_authority, _) = Pubkey::find_program_address(&[b"__event_authority"], &PUMP_PROGRAM_ID);
        let accounts = vec![
            AccountMeta::new(self.address, false),
            AccountMeta::new(user_accounts.owner, true),
            AccountMeta::new_readonly(global_config_address, false),
            AccountMeta::new_readonly(self.mint_a, false),
            AccountMeta::new_readonly(self.mint_b, false),
            AccountMeta::new(user_base_token_account, false),
            AccountMeta::new(user_quote_token_account, false),
            AccountMeta::new(self.vault_a, false),
            AccountMeta::new(self.vault_b, false),
            AccountMeta::new_readonly(*protocol_fee_recipient, false),
            AccountMeta::new(protocol_fee_recipient_token_account, false),
            AccountMeta::new_readonly(self.mint_a_program, false),
            AccountMeta::new_readonly(self.mint_b_program, false),
            AccountMeta::new_readonly(system_program::id(), false),
            AccountMeta::new_readonly(spl_associated_token_account::ID, false),
            AccountMeta::new_readonly(event_authority, false),
            AccountMeta::new_readonly(PUMP_PROGRAM_ID, false),
            AccountMeta::new(coin_creator_vault_ata, false),
            AccountMeta::new_readonly(coin_creator_vault_authority, false),
            AccountMeta::new(global_volume_accumulator, false),
            AccountMeta::new(user_volume_accumulator, false),
            AccountMeta::new_readonly(fee_config, false),
            AccountMeta::new_readonly(PUMP_FEE_PROGRAM_ID, false),
        ];
        Ok(Instruction {
            program_id: PUMP_PROGRAM_ID,
            accounts,
            data: instruction_data,
        })
    }
}

/// Construit l'instruction pour initialiser le compte de volume d'un utilisateur pour le programme pump.fun.
/// Ce compte est un prérequis pour que le programme accepte les transactions de l'utilisateur.
pub fn create_init_user_volume_accumulator_instruction(user_owner: &Pubkey) -> Result<Instruction> {
    let discriminator: [u8; 8] = [94, 6, 202, 115, 255, 96, 232, 183];
    let (user_volume_accumulator, _) = Pubkey::find_program_address(&[b"user_volume_accumulator", user_owner.as_ref()], &PUMP_PROGRAM_ID);
    let (event_authority, _) = Pubkey::find_program_address(&[b"__event_authority"], &PUMP_PROGRAM_ID);

    let accounts = vec![
        AccountMeta::new(*user_owner, true),
        AccountMeta::new_readonly(*user_owner, false),
        AccountMeta::new(user_volume_accumulator, false),
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