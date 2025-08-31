
use super::math;
use bytemuck::{pod_read_unaligned, Pod, Zeroable};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::program_pack::Pack;
use solana_sdk::pubkey::Pubkey;
use spl_token::state::{Account as SplTokenAccount, Mint as SplMint};
use anyhow::{anyhow, bail, Result};
use solana_sdk::instruction::{Instruction, AccountMeta};
use serde::{Serialize, Deserialize};
use async_trait::async_trait;
use crate::decoders::pool_operations::{PoolOperations, UserSwapAccounts};
use uint::construct_uint;



// --- CONSTANTES ---

pub const PROGRAM_ID: Pubkey = solana_sdk::pubkey!("Eo7WjKq67rjJQSZxS6z3YkapzY3eMj6Xy8X5EQVn5UaB");
pub const VAULT_PROGRAM_ID: Pubkey = solana_sdk::pubkey!("24Uqj9JCLxUeoC3hGfh5W3s9FM9uCHDS2SG3LYwBpyTi");
const POOL_STATE_DISCRIMINATOR: [u8; 8] = [241, 154, 109, 4, 17, 177, 109, 188];

// --- MODULE UNIQUE POUR TOUTES LES STRUCTURES ON-CHAIN ---
mod onchain_layouts {
    use super::*;

    // --- Structures pour le Vault ---
    #[repr(C, packed)]
    #[derive(Clone, Copy, Pod, Zeroable, Debug, Default, Serialize, Deserialize)]
    pub struct VaultBumps {
        pub vault_bump: u8,
        pub token_vault_bump: u8,
    }

    #[repr(C, packed)]
    #[derive(Clone, Copy, Pod, Zeroable, Debug, Default, Serialize, Deserialize)]
    pub struct LockedProfitTracker {
        pub last_updated_locked_profit: u64,
        pub last_report: u64,
        pub locked_profit_degradation: u64,
    }

    #[repr(C, packed)]
    #[derive(Clone, Copy, Pod, Zeroable, Debug, Default, Serialize, Deserialize)]
    pub struct Vault {
        pub enabled: u8,
        pub bumps: VaultBumps,
        pub total_amount: u64,
        pub token_vault: Pubkey,
        pub fee_vault: Pubkey,
        pub token_mint: Pubkey,
        pub lp_mint: Pubkey,
        pub strategies: [Pubkey; 30],
        pub base: Pubkey,
        pub admin: Pubkey,
        pub operator: Pubkey,
        pub locked_profit_tracker: LockedProfitTracker,
    }

    // --- Structures pour le Pool ---
    #[repr(C, packed)]
    #[derive(Clone, Copy, Pod, Zeroable, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
    pub struct TokenMultiplier {
        pub token_a_multiplier: u64,
        pub token_b_multiplier: u64,
        pub precision_factor: u8
    }

    #[repr(C, packed)]
    #[derive(Clone, Copy, Pod, Zeroable, Debug, Default, Serialize, Deserialize)]
    pub struct PoolFees {
        pub trade_fee_numerator: u64,
        pub trade_fee_denominator: u64,
        pub protocol_trade_fee_numerator: u64,
        pub protocol_trade_fee_denominator: u64
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
    #[repr(u8)]
    pub enum DepegType {
        None,
        Marinade,
        Lido,
        SplStake,
    }

    impl Default for DepegType {
        fn default() -> Self {
            DepegType::None
        }
    }

    #[repr(C, packed)]
    #[derive(Clone, Copy, Pod, Zeroable, Debug, Default)]
    pub struct Depeg {
        pub base_virtual_price: u64,
        pub base_cache_updated: u64,
        pub depeg_type: u8,
    }

    #[repr(C, packed)]
    #[derive(Clone, Copy, Pod, Zeroable, Debug, Default)]
    pub struct Pool {
        pub lp_mint: Pubkey,
        pub token_a_mint: Pubkey,
        pub token_b_mint: Pubkey,
        pub a_vault: Pubkey,
        pub b_vault: Pubkey,
        pub a_vault_lp: Pubkey,
        pub b_vault_lp: Pubkey,
        pub a_vault_lp_bump: u8,
        pub enabled: u8,
        pub protocol_token_a_fee: Pubkey,
        pub protocol_token_b_fee: Pubkey,
        pub fee_last_updated_at: u64,
        pub _padding0: [u8; 24],
        pub fees: PoolFees,
        pub pool_type: u8,
        pub stake: Pubkey,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MeteoraCurveType {
    ConstantProduct,
    Stable {
        amp: u64,
        token_multiplier: onchain_layouts::TokenMultiplier,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecodedMeteoraSbpPool {
    pub address: Pubkey,
    pub mint_a: Pubkey,
    pub mint_b: Pubkey,
    pub vault_a: Pubkey,
    pub vault_b: Pubkey,
    pub a_token_vault: Pubkey,
    pub b_token_vault: Pubkey,
    pub a_vault_lp: Pubkey,
    pub b_vault_lp: Pubkey,
    pub a_vault_lp_mint: Pubkey,
    pub b_vault_lp_mint: Pubkey,
    pub reserve_a: u64, // Continuera à stocker le montant calculé pour info
    pub reserve_b: u64, // Continuera à stocker le montant calculé pour info

    // --- NOUVEAUX CHAMPS POUR UN CALCUL PRÉCIS ---
    pub vault_a_state: onchain_layouts::Vault,
    pub vault_b_state: onchain_layouts::Vault,
    pub vault_a_lp_supply: u64,
    pub vault_b_lp_supply: u64,
    pub pool_a_vault_lp_amount: u64,
    pub pool_b_vault_lp_amount: u64,
    // --- FIN ---

    pub mint_a_decimals: u8,
    pub mint_b_decimals: u8,
    pub fees: onchain_layouts::PoolFees,
    pub curve_type: MeteoraCurveType,
    pub enabled: bool,
    pub stake: Pubkey,
    pub last_swap_timestamp: i64,
}


fn read_pod<T: Pod>(data: &[u8], offset: usize) -> Result<T> {
    let size = std::mem::size_of::<T>();
    let end = offset.checked_add(size).ok_or_else(|| anyhow!("Offset overflow"))?;
    if end > data.len() { bail!("Buffer underflow reading at offset {}", offset); }
    Ok(pod_read_unaligned(&data[offset..end]))
}

pub fn decode_pool(address: &Pubkey, data: &[u8]) -> Result<DecodedMeteoraSbpPool> {
    if data.get(..8) != Some(&POOL_STATE_DISCRIMINATOR) { bail!("Invalid discriminator."); }
    let data_slice = &data[8..];

    let pool_struct: &onchain_layouts::Pool = bytemuck::from_bytes(&data_slice[..std::mem::size_of::<onchain_layouts::Pool>()]);

    const CURVE_TYPE_OFFSET: usize = 866;
    let curve_kind: u8 = read_pod(data_slice, CURVE_TYPE_OFFSET)?;
    let curve_type = match curve_kind {
        0 => MeteoraCurveType::ConstantProduct,
        1 => {
            const STABLE_PARAMS_OFFSET: usize = CURVE_TYPE_OFFSET + 1;
            let amp: u64 = read_pod(data_slice, STABLE_PARAMS_OFFSET)?;
            let token_multiplier: onchain_layouts::TokenMultiplier = read_pod(data_slice, STABLE_PARAMS_OFFSET + 8)?;
            MeteoraCurveType::Stable { amp, token_multiplier }
        }
        _ => bail!("Unsupported curve type: {}", curve_kind),
    };

    Ok(DecodedMeteoraSbpPool {
        address: *address,
        mint_a: pool_struct.token_a_mint,
        mint_b: pool_struct.token_b_mint,
        vault_a: pool_struct.a_vault,
        vault_b: pool_struct.b_vault,
        a_vault_lp: pool_struct.a_vault_lp,
        b_vault_lp: pool_struct.b_vault_lp,
        stake: pool_struct.stake,
        enabled: pool_struct.enabled == 1, // Conversion de u8 en bool
        fees: pool_struct.fees,
        curve_type,
        // Champs à hydrater
        a_token_vault: Pubkey::default(),
        b_token_vault: Pubkey::default(),
        a_vault_lp_mint: Pubkey::default(),
        b_vault_lp_mint: Pubkey::default(),
        reserve_a: 0, reserve_b: 0, mint_a_decimals: 0, mint_b_decimals: 0,
        vault_a_state: onchain_layouts::Vault::default(),
        vault_b_state: onchain_layouts::Vault::default(),
        vault_a_lp_supply: 0,
        vault_b_lp_supply: 0,
        pool_a_vault_lp_amount: 0,
        pool_b_vault_lp_amount: 0,
        last_swap_timestamp: 0,
    })
}

pub async fn hydrate(pool: &mut DecodedMeteoraSbpPool, rpc_client: &RpcClient) -> Result<()> {
    // Étape 1 : Récupérer les données des comptes principaux (inchangé)
    let (
        vault_a_res, vault_b_res, pool_lp_a_res, pool_lp_b_res, mint_a_res, mint_b_res
    ) = tokio::join!(
        rpc_client.get_account_data(&pool.vault_a),
        rpc_client.get_account_data(&pool.vault_b),
        rpc_client.get_account_data(&pool.a_vault_lp),
        rpc_client.get_account_data(&pool.b_vault_lp),
        rpc_client.get_account_data(&pool.mint_a),
        rpc_client.get_account_data(&pool.mint_b)
    );

    // Étape 2 : Décoder les données des vaults et stocker les structures complètes (inchangé)
    let vault_a_data = vault_a_res?;
    let vault_b_data = vault_b_res?;
    let expected_size = std::mem::size_of::<onchain_layouts::Vault>();
    if vault_a_data.len() < 8 + expected_size || vault_b_data.len() < 8 + expected_size {
        bail!("Les données du compte Vault sont trop courtes.");
    }
    let data_slice_a = &vault_a_data[8..];
    let vault_a_struct: &onchain_layouts::Vault = bytemuck::from_bytes(&data_slice_a[..expected_size]);
    pool.vault_a_state = *vault_a_struct;
    pool.a_token_vault = vault_a_struct.token_vault;
    pool.a_vault_lp_mint = vault_a_struct.lp_mint;

    let data_slice_b = &vault_b_data[8..];
    let vault_b_struct: &onchain_layouts::Vault = bytemuck::from_bytes(&data_slice_b[..expected_size]);
    pool.vault_b_state = *vault_b_struct;
    pool.b_token_vault = vault_b_struct.token_vault;
    pool.b_vault_lp_mint = vault_b_struct.lp_mint;

    // Étape 3 : Récupérer les données des mints LP des vaults (inchangé)
    let (vault_a_lp_mint_res, vault_b_lp_mint_res) = tokio::join!(
        rpc_client.get_account_data(&pool.a_vault_lp_mint),
        rpc_client.get_account_data(&pool.b_vault_lp_mint)
    );

    // Étape 4 : Décoder les données restantes (inchangé)
    pool.mint_a_decimals = crate::decoders::spl_token_decoders::mint::decode_mint(&pool.mint_a, &mint_a_res?)?.decimals;
    pool.mint_b_decimals = crate::decoders::spl_token_decoders::mint::decode_mint(&pool.mint_b, &mint_b_res?)?.decimals;

    let pool_lp_a_account = SplTokenAccount::unpack(&pool_lp_a_res?)?;
    pool.pool_a_vault_lp_amount = pool_lp_a_account.amount;
    let pool_lp_b_account = SplTokenAccount::unpack(&pool_lp_b_res?)?;
    pool.pool_b_vault_lp_amount = pool_lp_b_account.amount;

    let vault_a_lp_mint_account = SplMint::unpack(&vault_a_lp_mint_res?)?;
    pool.vault_a_lp_supply = vault_a_lp_mint_account.supply;
    let vault_b_lp_mint_account = SplMint::unpack(&vault_b_lp_mint_res?)?;
    pool.vault_b_lp_supply = vault_b_lp_mint_account.supply;

    // --- DÉBUT DE LA CORRECTION FINALE ---
    // Étape 5 : Calculer les réserves en utilisant le MONTANT DÉVERROUILLÉ

    // Pour cela, nous avons besoin du timestamp actuel. Nous allons le récupérer.
    let clock_account = rpc_client.get_account(&solana_sdk::sysvar::clock::ID).await?;
    let clock: solana_sdk::sysvar::clock::Clock = bincode::deserialize(&clock_account.data)?;
    let current_timestamp = clock.unix_timestamp;

    let unlocked_a = get_unlocked_amount(&pool.vault_a_state, current_timestamp);
    let unlocked_b = get_unlocked_amount(&pool.vault_b_state, current_timestamp);

    if pool.vault_a_lp_supply > 0 {
        pool.reserve_a = get_tokens_for_lp(pool.pool_a_vault_lp_amount, unlocked_a, pool.vault_a_lp_supply);
    }
    if pool.vault_b_lp_supply > 0 {
        pool.reserve_b = get_tokens_for_lp(pool.pool_b_vault_lp_amount, unlocked_b, pool.vault_b_lp_supply);
    }
    // --- FIN DE LA CORRECTION FINALE ---

    Ok(())
}

const LOCKED_PROFIT_DEGRADATION_DENOMINATOR: u128 = 1_000_000_000_000;

fn calculate_locked_profit(tracker: &onchain_layouts::LockedProfitTracker, current_time: u64) -> u64 {
    let duration = u128::from(current_time.saturating_sub(tracker.last_report));
    let degradation = u128::from(tracker.locked_profit_degradation);
    let ratio = duration.saturating_mul(degradation);
    if ratio > LOCKED_PROFIT_DEGRADATION_DENOMINATOR { return 0; }
    let locked_profit = u128::from(tracker.last_updated_locked_profit);
    ((locked_profit.saturating_mul(LOCKED_PROFIT_DEGRADATION_DENOMINATOR - ratio)) / LOCKED_PROFIT_DEGRADATION_DENOMINATOR) as u64
}

fn get_unlocked_amount(vault: &onchain_layouts::Vault, current_time: i64) -> u64 {
    vault.total_amount.saturating_sub(calculate_locked_profit(&vault.locked_profit_tracker, current_time as u64))
}

fn get_lp_for_tokens(token_amount: u64, vault_total_tokens: u64, vault_lp_supply: u64) -> u64 {
    if vault_total_tokens == 0 { return token_amount; } // Bootstrap case
    ((token_amount as u128).saturating_mul(vault_lp_supply as u128) / vault_total_tokens as u128) as u64
}

fn get_tokens_for_lp(lp_amount: u64, vault_total_tokens: u64, vault_lp_supply: u64) -> u64 {
    if vault_lp_supply == 0 { return 0; }
    ((lp_amount as u128).saturating_mul(vault_total_tokens as u128) / vault_lp_supply as u128) as u64
}


// DANS : src/decoders/meteora_decoders/pool
// REMPLACEZ TOUT LE BLOC `impl PoolOperations for DecodedMeteoraSbpPool`
#[async_trait]
impl PoolOperations for DecodedMeteoraSbpPool {
    fn get_mints(&self) -> (Pubkey, Pubkey) { (self.mint_a, self.mint_b) }
    fn get_vaults(&self) -> (Pubkey, Pubkey) { (self.vault_a, self.vault_b) }
    fn address(&self) -> Pubkey { self.address }

    fn get_quote(&self, token_in_mint: &Pubkey, amount_in: u64, _current_timestamp: i64) -> Result<u64> {
        if !self.enabled || amount_in == 0 { return Ok(0); }

        let (in_reserve, out_reserve) = if *token_in_mint == self.mint_a {
            (self.reserve_a, self.reserve_b)
        } else {
            (self.reserve_b, self.reserve_a)
        };

        if in_reserve == 0 || out_reserve == 0 { return Ok(0); }

        let fees = &self.fees;

        // --- DÉBUT DE LA LOGIQUE DE FRAIS UNIFIÉE ET CORRECTE ---

        // Étape 1 : Calculer le total des frais (LP + Protocole) sur le montant d'entrée.
        let total_fee_numerator = fees.trade_fee_numerator.saturating_add(fees.protocol_trade_fee_numerator);
        let fee_denominator = fees.trade_fee_denominator;
        if fee_denominator == 0 { return Ok(0); }

        let total_fee = (amount_in as u128)
            .checked_mul(total_fee_numerator as u128)
            .ok_or_else(|| anyhow!("Math overflow"))?
            .checked_div(fee_denominator as u128)
            .ok_or_else(|| anyhow!("Math overflow"))?;

        // Étape 2 : Soustraire les frais pour obtenir le montant net qui sera réellement échangé.
        let amount_in_after_fee = (amount_in as u128).saturating_sub(total_fee);

        // Étape 3 : Appliquer la bonne formule mathématique sur le montant net.
        let amount_out = match &self.curve_type {
            MeteoraCurveType::ConstantProduct => {
                let numerator = amount_in_after_fee.saturating_mul(out_reserve as u128);
                let denominator = (in_reserve as u128).saturating_add(amount_in_after_fee);
                if denominator == 0 { 0 } else { (numerator / denominator) as u64 }
            },
            MeteoraCurveType::Stable { amp, token_multiplier } => {
                let multiplier = *token_multiplier;
                let (norm_in_reserve, norm_out_reserve, in_mult, out_mult) = if *token_in_mint == self.mint_a {
                    ((in_reserve as u128 * multiplier.token_a_multiplier as u128), (out_reserve as u128 * multiplier.token_b_multiplier as u128), multiplier.token_a_multiplier, multiplier.token_b_multiplier)
                } else {
                    ((in_reserve as u128 * multiplier.token_b_multiplier as u128), (out_reserve as u128 * multiplier.token_a_multiplier as u128), multiplier.token_b_multiplier, multiplier.token_a_multiplier)
                };

                let norm_in = amount_in_after_fee.saturating_mul(in_mult as u128);
                let norm_out = math::get_quote(norm_in as u64, norm_in_reserve as u64, norm_out_reserve as u64, *amp)? as u128;

                norm_out.checked_div(out_mult as u128).unwrap_or(0) as u64
            }
        };

        Ok(amount_out)
    }

    fn get_required_input(
        &self,
        token_out_mint: &Pubkey,
        amount_out: u64,
        _current_timestamp: i64,
    ) -> Result<u64> {
        if !self.enabled || amount_out == 0 { return Ok(0); }

        let (in_reserve, out_reserve) = if *token_out_mint == self.mint_b {
            (self.reserve_a, self.reserve_b)
        } else {
            (self.reserve_b, self.reserve_a)
        };

        if out_reserve == 0 || in_reserve == 0 || amount_out >= out_reserve {
            return Err(anyhow!("Invalid reserves or amount_out too high for DAMM v1."));
        }

        // --- 1. Calculer le montant d'input NET nécessaire ---
        let amount_in_after_fee = match &self.curve_type {
            MeteoraCurveType::ConstantProduct => {
                let numerator = (amount_out as u128).saturating_mul(in_reserve as u128);
                let denominator = (out_reserve as u128).saturating_sub(amount_out as u128);
                // Arrondi au plafond
                numerator.checked_add(denominator.saturating_sub(1)).ok_or_else(||anyhow!("Math overflow"))?
                    .checked_div(denominator).ok_or_else(||anyhow!("Math overflow"))? as u64
            },
            MeteoraCurveType::Stable { amp, token_multiplier } => {
                let multiplier = *token_multiplier;
                let (norm_in_reserve, norm_out_reserve, in_mult, out_mult) = if *token_out_mint == self.mint_b {
                    ((in_reserve as u128 * multiplier.token_a_multiplier as u128), (out_reserve as u128 * multiplier.token_b_multiplier as u128), multiplier.token_a_multiplier, multiplier.token_b_multiplier)
                } else {
                    ((in_reserve as u128 * multiplier.token_b_multiplier as u128), (out_reserve as u128 * multiplier.token_a_multiplier as u128), multiplier.token_b_multiplier, multiplier.token_a_multiplier)
                };

                let norm_out = (amount_out as u128).saturating_mul(out_mult as u128);
                let norm_in = math::get_required_input(norm_out as u64, norm_in_reserve as u64, norm_out_reserve as u64, *amp)? as u128;

                // Arrondi au plafond
                norm_in.checked_add((in_mult as u128).saturating_sub(1)).ok_or_else(||anyhow!("Math overflow"))?
                    .checked_div(in_mult as u128).ok_or_else(||anyhow!("Math overflow"))? as u64
            }
        };

        // --- 2. Inverser les frais de pool ---
        let fees = &self.fees;
        let total_fee_numerator = fees.trade_fee_numerator.saturating_add(fees.protocol_trade_fee_numerator);
        let fee_denominator = fees.trade_fee_denominator;

        if total_fee_numerator == 0 {
            return Ok(amount_in_after_fee);
        }

        let numerator = (amount_in_after_fee as u128).saturating_mul(fee_denominator as u128);
        let denominator = (fee_denominator as u128).saturating_sub(total_fee_numerator as u128);

        // Arrondi au plafond
        let required_amount_in = numerator.checked_add(denominator.saturating_sub(1)).ok_or_else(||anyhow!("Math overflow"))?
            .checked_div(denominator).ok_or_else(||anyhow!("Math overflow"))?;

        Ok(required_amount_in as u64)
    }

    async fn get_quote_async(&mut self, token_in_mint: &Pubkey, amount_in: u64, _rpc_client: &RpcClient) -> Result<u64> {
        self.get_quote(token_in_mint, amount_in, 0)
    }
    fn create_swap_instruction(
        &self,
        token_in_mint: &Pubkey,
        amount_in: u64,
        min_amount_out: u64,
        user_accounts: &UserSwapAccounts,
    ) -> Result<Instruction> {
        let instruction_discriminator: [u8; 8] = [248, 198, 158, 145, 225, 117, 135, 200];
        let mut instruction_data = Vec::with_capacity(8 + 8 + 8);
        instruction_data.extend_from_slice(&instruction_discriminator);
        instruction_data.extend_from_slice(&amount_in.to_le_bytes());
        instruction_data.extend_from_slice(&min_amount_out.to_le_bytes());

        let protocol_fee = if *token_in_mint == self.mint_a {
            Pubkey::find_program_address(&[b"fee", self.mint_a.as_ref(), self.address.as_ref()], &PROGRAM_ID).0
        } else {
            Pubkey::find_program_address(&[b"fee", self.mint_b.as_ref(), self.address.as_ref()], &PROGRAM_ID).0
        };

        let mut accounts = vec![
            AccountMeta::new(self.address, false),
            AccountMeta::new(user_accounts.source, false),
            AccountMeta::new(user_accounts.destination, false),
            AccountMeta::new(self.vault_a, false),
            AccountMeta::new(self.vault_b, false),
            AccountMeta::new(self.a_token_vault, false),
            AccountMeta::new(self.b_token_vault, false),
            AccountMeta::new(self.a_vault_lp_mint, false),
            AccountMeta::new(self.b_vault_lp_mint, false),
            AccountMeta::new(self.a_vault_lp, false),
            AccountMeta::new(self.b_vault_lp, false),
            AccountMeta::new(protocol_fee, false),
            AccountMeta::new_readonly(user_accounts.owner, true),
            AccountMeta::new_readonly(VAULT_PROGRAM_ID, false),
            AccountMeta::new_readonly(spl_token::id(), false),
        ];

        if let MeteoraCurveType::Stable {..} = self.curve_type {
            if self.stake != Pubkey::default() {
                accounts.push(AccountMeta::new_readonly(self.stake, false));
            }
        }

        Ok(Instruction {
            program_id: PROGRAM_ID,
            accounts,
            data: instruction_data,
        })
    }
}

impl DecodedMeteoraSbpPool {
    pub fn fee_as_percent(&self) -> f64 {
        if self.fees.trade_fee_denominator == 0 { return 0.0; }
        (self.fees.trade_fee_numerator as f64 * 100.0) / self.fees.trade_fee_denominator as f64
    }
}