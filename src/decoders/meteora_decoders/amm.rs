// DANS: src/decoders/meteora_decoders/amm.rs

use crate::decoders::pool_operations::PoolOperations;
use crate::math::meteora_stableswap_math;
use bytemuck::{pod_read_unaligned, Pod, Zeroable};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::program_pack::Pack;
use solana_sdk::pubkey::Pubkey;
use spl_token::state::{Account as SplTokenAccount, Mint as SplMint};
use anyhow::{anyhow, bail, Result};
use solana_sdk::instruction::{Instruction, AccountMeta};




// --- CONSTANTES ET STRUCTURES PUBLIQUES ---

pub const PROGRAM_ID: Pubkey = solana_sdk::pubkey!("Eo7WjKq67rjJQSZxS6z3YkapzY3eMj6Xy8X5EQVn5UaB");
pub const VAULT_PROGRAM_ID: Pubkey = solana_sdk::pubkey!("24Uqj9JCLxUeoC3hGfh5W3s9FM9uCHDS2SG3LYwBpyTi");
const POOL_STATE_DISCRIMINATOR: [u8; 8] = [241, 154, 109, 4, 17, 177, 109, 188];


mod onchain_vault_layouts {
    use super::*;

    #[repr(C, packed)]
    #[derive(Clone, Copy, Pod, Zeroable, Debug, Default)]
    pub struct VaultBumps {
        pub vault_bump: u8,
        pub token_vault_bump: u8,
    }

    #[repr(C, packed)]
    #[derive(Clone, Copy, Pod, Zeroable, Debug, Default)]
    pub struct LockedProfitTracker {
        pub last_updated_locked_profit: u64,
        pub last_report: u64,
        pub locked_profit_degradation: u64,
    }

    #[repr(C, packed)]
    #[derive(Clone, Copy, Pod, Zeroable, Debug, Default)]
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MeteoraCurveType {
    ConstantProduct,
    Stable {
        amp: u64,
        token_multiplier: onchain_layouts::TokenMultiplier,
    },
}

#[derive(Debug, Clone)]
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
    pub vault_a_state: onchain_vault_layouts::Vault,
    pub vault_b_state: onchain_vault_layouts::Vault,
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
}

mod onchain_layouts {
    use super::*;
    #[repr(C, packed)]
    #[derive(Clone, Copy, Pod, Zeroable, Debug, PartialEq, Eq)]
    pub struct TokenMultiplier { pub token_a_multiplier: u64, pub token_b_multiplier: u64, pub precision_factor: u8 }
    #[repr(C, packed)]
    #[derive(Clone, Copy, Pod, Zeroable, Debug)]
    pub struct PoolFees { pub trade_fee_numerator: u64, pub trade_fee_denominator: u64, pub protocol_trade_fee_numerator: u64, pub protocol_trade_fee_denominator: u64 }
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
    let token_a_mint: Pubkey = read_pod(data_slice, 32)?;
    let token_b_mint: Pubkey = read_pod(data_slice, 64)?;
    let a_vault: Pubkey = read_pod(data_slice, 96)?;
    let b_vault: Pubkey = read_pod(data_slice, 128)?;
    let a_vault_lp: Pubkey = read_pod(data_slice, 160)?;
    let b_vault_lp: Pubkey = read_pod(data_slice, 192)?;
    let enabled: u8 = read_pod(data_slice, 225)?;
    const FEES_OFFSET: usize = 322;
    let fees: onchain_layouts::PoolFees = read_pod(data_slice, FEES_OFFSET)?;
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
        mint_a: token_a_mint, mint_b: token_b_mint,
        vault_a: a_vault, vault_b: b_vault,
        a_token_vault: Pubkey::default(),
        b_token_vault: Pubkey::default(),
        a_vault_lp, b_vault_lp,
        vault_a_state: onchain_vault_layouts::Vault::default(), // Correction
        vault_b_state: onchain_vault_layouts::Vault::default(), // Initialisation à zéro
        vault_a_lp_supply: 0,
        vault_b_lp_supply: 0,
        pool_a_vault_lp_amount: 0,
        pool_b_vault_lp_amount: 0,
        a_vault_lp_mint: Pubkey::default(),
        b_vault_lp_mint: Pubkey::default(),
        fees, curve_type, enabled: enabled == 1,
        reserve_a: 0, reserve_b: 0, mint_a_decimals: 0, mint_b_decimals: 0,
    })
}

pub async fn hydrate(pool: &mut DecodedMeteoraSbpPool, rpc_client: &RpcClient) -> Result<()> {
    // Étape 1 : Lire les comptes Vault pour découvrir les adresses des LP mints
    let (vault_a_res, vault_b_res) = tokio::join!(
        rpc_client.get_account_data(&pool.vault_a),
        rpc_client.get_account_data(&pool.vault_b)
    );

    let vault_a_data = vault_a_res?;
    let vault_b_data = vault_b_res?;

    let expected_size = std::mem::size_of::<onchain_vault_layouts::Vault>();
    if vault_a_data.len() < 8 + expected_size || vault_b_data.len() < 8 + expected_size {
        bail!("Les données du compte Vault sont trop courtes.");
    }

    let data_slice_a = &vault_a_data[8..];
    let vault_a_struct: &onchain_vault_layouts::Vault = bytemuck::from_bytes(&data_slice_a[..expected_size]);
    pool.a_token_vault = vault_a_struct.token_vault;
    pool.a_vault_lp_mint = vault_a_struct.lp_mint;
    pool.vault_a_state = *vault_a_struct; // On copie la structure entière

    let data_slice_b = &vault_b_data[8..];
    let vault_b_struct: &onchain_vault_layouts::Vault = bytemuck::from_bytes(&data_slice_b[..expected_size]);
    pool.b_token_vault = vault_b_struct.token_vault;
    pool.b_vault_lp_mint = vault_b_struct.lp_mint;
    pool.vault_b_state = *vault_b_struct; // On copie la structure entière

    // Étape 2 : Maintenant, faire un seul gros appel pour tout le reste
    let (pool_lp_a_res, pool_lp_b_res, mint_a_res, mint_b_res, vault_a_lp_mint_res, vault_b_lp_mint_res) = tokio::join!(
        rpc_client.get_account_data(&pool.a_vault_lp),
        rpc_client.get_account_data(&pool.b_vault_lp),
        rpc_client.get_account_data(&pool.mint_a),
        rpc_client.get_account_data(&pool.mint_b),
        rpc_client.get_account_data(&pool.a_vault_lp_mint),
        rpc_client.get_account_data(&pool.b_vault_lp_mint)
    );

    // Étape 3 : Décoder et assigner
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

    // Étape 4 : Calculer les réserves finales
    if pool.vault_a_lp_supply > 0 {
        pool.reserve_a = ((pool.vault_a_state.total_amount as u128 * pool.pool_a_vault_lp_amount as u128) / pool.vault_a_lp_supply as u128) as u64;
    }
    if pool.vault_b_lp_supply > 0 {
        pool.reserve_b = ((pool.vault_b_state.total_amount as u128 * pool.pool_b_vault_lp_amount as u128) / pool.vault_b_lp_supply as u128) as u64;
    }

    Ok(())
}

const LOCKED_PROFIT_DEGRADATION_DENOMINATOR: u128 = 1_000_000_000_000;

fn calculate_locked_profit(tracker: &onchain_vault_layouts::LockedProfitTracker, current_time: u64) -> u64 {
    let duration = u128::from(current_time.saturating_sub(tracker.last_report));
    let degradation = u128::from(tracker.locked_profit_degradation);
    let ratio = duration.saturating_mul(degradation);
    if ratio > LOCKED_PROFIT_DEGRADATION_DENOMINATOR { return 0; }
    let locked_profit = u128::from(tracker.last_updated_locked_profit);
    ((locked_profit.saturating_mul(LOCKED_PROFIT_DEGRADATION_DENOMINATOR - ratio)) / LOCKED_PROFIT_DEGRADATION_DENOMINATOR) as u64
}

fn get_unlocked_amount(vault: &onchain_vault_layouts::Vault, current_time: i64) -> u64 {
    vault.total_amount.saturating_sub(calculate_locked_profit(&vault.locked_profit_tracker, current_time as u64))
}

fn get_unmint_amount(unlocked_amount: u64, out_token: u64, lp_supply: u64) -> u64 {
    if unlocked_amount == 0 { return 0; }
    ((out_token as u128).saturating_mul(lp_supply as u128) / unlocked_amount as u128) as u64
}

impl PoolOperations for DecodedMeteoraSbpPool {
    fn get_mints(&self) -> (Pubkey, Pubkey) { (self.mint_a, self.mint_b) }
    fn get_vaults(&self) -> (Pubkey, Pubkey) { (self.vault_a, self.vault_b) }

    fn get_quote(&self, token_in_mint: &Pubkey, amount_in: u64, current_timestamp: i64) -> Result<u64> {
        if !self.enabled || amount_in == 0 { return Ok(0); }

        let (in_vault_state, out_vault_state, in_vault_lp_supply, out_vault_lp_supply, pool_in_vault_lp, _pool_out_vault_lp, in_reserve, out_reserve) =
            if *token_in_mint == self.mint_a {
                (&self.vault_a_state, &self.vault_b_state, self.vault_a_lp_supply, self.vault_b_lp_supply, self.pool_a_vault_lp_amount, self.pool_b_vault_lp_amount, self.reserve_a, self.reserve_b)
            } else {
                (&self.vault_b_state, &self.vault_a_state, self.vault_b_lp_supply, self.vault_a_lp_supply, self.pool_b_vault_lp_amount, self.pool_a_vault_lp_amount, self.reserve_b, self.reserve_a)
            };

        if in_reserve == 0 || out_reserve == 0 { return Ok(0); }

        let fees = &self.fees;

        // --- DÉBUT DE LA LOGIQUE CORRIGÉE 1:1 AVEC LE SDK ---

        // Étape 1 : Calculer les frais.
        let trade_fee = ((amount_in as u128 * fees.trade_fee_numerator as u128) / fees.trade_fee_denominator as u128) as u64;
        let protocol_fee = ((trade_fee as u128 * fees.protocol_trade_fee_numerator as u128) / fees.protocol_trade_fee_denominator as u128) as u64;
        let lp_trade_fee = trade_fee.saturating_sub(protocol_fee);

        // Le SDK soustrait d'abord les frais de protocole.
        let in_amount_after_protocol_fee = amount_in.saturating_sub(protocol_fee);

        // Étape 2 : Simuler le dépôt dans le vault pour obtenir le montant "réel" qui entre dans le pool.
        let in_vault_unlocked = get_unlocked_amount(in_vault_state, current_timestamp);
        let in_lp_minted = get_unmint_amount(in_vault_unlocked, in_amount_after_protocol_fee, in_vault_lp_supply);

        let after_deposit_in_vault_unlocked = in_vault_unlocked.saturating_add(in_amount_after_protocol_fee);
        let after_deposit_in_vault_lp_supply = in_vault_lp_supply.saturating_add(in_lp_minted);
        let after_deposit_pool_in_vault_lp = pool_in_vault_lp.saturating_add(in_lp_minted);

        let after_deposit_in_reserve = if after_deposit_in_vault_lp_supply == 0 { 0 } else {
            ((after_deposit_pool_in_vault_lp as u128 * after_deposit_in_vault_unlocked as u128) / after_deposit_in_vault_lp_supply as u128) as u64
        };

        let actual_in_amount = after_deposit_in_reserve.saturating_sub(in_reserve);

        // Étape 3 : Soustraire les frais de LP du montant réel.
        let amount_in_after_lp_fee = actual_in_amount.saturating_sub(lp_trade_fee);

        // Étape 4 : Calculer le swap avec le montant net réel.
        let gross_amount_out = match &self.curve_type {
            MeteoraCurveType::ConstantProduct => {
                let numerator = (amount_in_after_lp_fee as u128).saturating_mul(out_reserve as u128);
                let denominator = (in_reserve as u128).saturating_add(amount_in_after_lp_fee as u128);
                if denominator == 0 { 0 } else { (numerator / denominator) as u64 }
            },
            MeteoraCurveType::Stable { amp, token_multiplier } => {
                // Pour les stables, les frais s'appliquent sur la sortie. La logique est différente.
                // Nous allons ignorer la complexité du vault pour l'instant et revenir à une logique plus simple et correcte pour les stables.
                let total_fee_numerator = fees.trade_fee_numerator.saturating_add(fees.protocol_trade_fee_numerator);
                let multiplier = *token_multiplier;
                let (norm_in_reserve, norm_out_reserve, in_mult, out_mult) = if *token_in_mint == self.mint_a {
                    (in_reserve as u128 * multiplier.token_a_multiplier as u128, out_reserve as u128 * multiplier.token_b_multiplier as u128, multiplier.token_a_multiplier, multiplier.token_b_multiplier)
                } else {
                    (in_reserve as u128 * multiplier.token_b_multiplier as u128, out_reserve as u128 * multiplier.token_a_multiplier as u128, multiplier.token_b_multiplier, multiplier.token_a_multiplier)
                };

                let norm_in = (amount_in as u128).saturating_mul(in_mult as u128);
                let norm_out = meteora_stableswap_math::get_quote(norm_in as u64, norm_in_reserve as u64, norm_out_reserve as u64, *amp)? as u128;

                let amount_out_before_fee = norm_out.checked_div(out_mult as u128).unwrap_or(0);

                let fee = amount_out_before_fee
                    .checked_mul(total_fee_numerator as u128)
                    .ok_or_else(|| anyhow!("Math overflow"))?
                    .checked_div(fees.trade_fee_denominator as u128)
                    .ok_or_else(|| anyhow!("Math overflow"))?;

                amount_out_before_fee.saturating_sub(fee) as u64
            }
        };

        // Étape 5 : Simuler le retrait du vault de sortie.
        let out_vault_unlocked = get_unlocked_amount(out_vault_state, current_timestamp);
        let out_lp_to_burn = get_unmint_amount(out_vault_unlocked, gross_amount_out, out_vault_lp_supply);

        let final_amount_out = if out_vault_lp_supply == 0 { 0 } else {
            ((out_lp_to_burn as u128 * out_vault_unlocked as u128) / out_vault_lp_supply as u128) as u64
        };

        // --- CORRECTION FINALE : Ne pas appliquer la logique du vault aux pools stables pour l'instant ---
        if let MeteoraCurveType::Stable { .. } = self.curve_type {
            return Ok(gross_amount_out); // On retourne le calcul direct pour les pools stables
        }

        Ok(final_amount_out)
    }
}

impl DecodedMeteoraSbpPool {
    pub fn fee_as_percent(&self) -> f64 {
        if self.fees.trade_fee_denominator == 0 { return 0.0; }
        (self.fees.trade_fee_numerator as f64 * 100.0) / self.fees.trade_fee_denominator as f64
    }

    pub fn create_swap_instruction(
        &self,
        token_in_mint: &Pubkey,
        user_source_token_account: &Pubkey,
        user_destination_token_account: &Pubkey,
        user_owner: &Pubkey,
        in_amount: u64,
        minimum_out_amount: u64,
    ) -> Result<Instruction> {
        let instruction_discriminator: [u8; 8] = [248, 198, 158, 145, 225, 117, 135, 200];

        let mut instruction_data = Vec::with_capacity(8 + 8 + 8);
        instruction_data.extend_from_slice(&instruction_discriminator);
        instruction_data.extend_from_slice(&in_amount.to_le_bytes());
        instruction_data.extend_from_slice(&minimum_out_amount.to_le_bytes());

        let protocol_fee =
            if *token_in_mint == self.mint_a {
                let (pda, _) = Pubkey::find_program_address(&[b"fee", self.mint_a.as_ref(), self.address.as_ref()], &PROGRAM_ID);
                pda
            } else {
                let (pda, _) = Pubkey::find_program_address(&[b"fee", self.mint_b.as_ref(), self.address.as_ref()], &PROGRAM_ID);
                pda
            };

        // L'ordre final et correct des comptes, basé sur l'IDL et les transactions on-chain.
        let accounts = vec![
            AccountMeta::new(self.address, false),
            AccountMeta::new(*user_source_token_account, false),
            AccountMeta::new(*user_destination_token_account, false),
            AccountMeta::new(self.vault_a, false),
            AccountMeta::new(self.vault_b, false),
            AccountMeta::new(self.a_token_vault, false),
            AccountMeta::new(self.b_token_vault, false),
            AccountMeta::new(self.a_vault_lp_mint, false), // MINT d'abord
            AccountMeta::new(self.b_vault_lp_mint, false), // MINT d'abord
            AccountMeta::new(self.a_vault_lp, false),      // PUIS le compte LP
            AccountMeta::new(self.b_vault_lp, false),      // PUIS le compte LP
            AccountMeta::new(protocol_fee, false),
            AccountMeta::new_readonly(*user_owner, true),
            AccountMeta::new_readonly(VAULT_PROGRAM_ID, false),
            AccountMeta::new_readonly(spl_token::id(), false),
        ];

        Ok(Instruction {
            program_id: PROGRAM_ID,
            accounts,
            data: instruction_data,
        })
    }
}