// src/decoders/meteora/damm_v1/pool.rs

use super::math;
use bytemuck::{Pod, Zeroable};
use solana_sdk::program_pack::Pack;
use solana_sdk::pubkey::Pubkey;
use spl_token::state::{Account as SplTokenAccount, Mint as SplMint};
use anyhow::{anyhow, bail, Result};
use solana_sdk::instruction::{Instruction, AccountMeta};
use serde::{Serialize, Deserialize};
use async_trait::async_trait;
use crate::decoders::pool_operations::{PoolOperations, UserSwapAccounts, QuoteResult};
use crate::decoders::pool_operations::find_input_by_binary_search;
use crate::rpc::ResilientRpcClient;
use crate::monitoring::metrics;
use std::time::Instant;
use tracing::debug;

// --- CONSTANTES ---
pub const PROGRAM_ID: Pubkey = solana_sdk::pubkey!("Eo7WjKq67rjJQSZxS6z3YkapzY3eMj6Xy8X5EQVn5UaB");
pub const VAULT_PROGRAM_ID: Pubkey = solana_sdk::pubkey!("24Uqj9JCLxUeoC3hGfh5W3s9FM9uCHDS2SG3LYwBpyTi");
const POOL_STATE_DISCRIMINATOR: [u8; 8] = [241, 154, 109, 4, 17, 177, 109, 188];

// --- MODULE POUR LES STRUCTURES ON-CHAIN ---
pub mod onchain_layouts {
    use super::*;

    // Les structs internes restent les mêmes
    #[repr(C, packed)] #[derive(Clone, Copy, Pod, Zeroable, Debug, Default, Serialize, Deserialize)]
    pub struct VaultBumps { pub vault_bump: u8, pub token_vault_bump: u8 }
    #[repr(C, packed)] #[derive(Clone, Copy, Pod, Zeroable, Debug, Default, Serialize, Deserialize)]
    pub struct LockedProfitTracker { pub last_updated_locked_profit: u64, pub last_report: u64, pub locked_profit_degradation: u64 }
    #[repr(C, packed)] #[derive(Clone, Copy, Pod, Zeroable, Debug, Default, Serialize, Deserialize)]
    pub struct Vault {
        pub enabled: u8, pub bumps: VaultBumps, pub total_amount: u64, pub token_vault: Pubkey,
        pub fee_vault: Pubkey, pub token_mint: Pubkey, pub lp_mint: Pubkey, pub strategies: [Pubkey; 30],
        pub base: Pubkey, pub admin: Pubkey, pub operator: Pubkey, pub locked_profit_tracker: LockedProfitTracker,
    }
    #[repr(C, packed)] #[derive(Clone, Copy, Pod, Zeroable, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
    pub struct TokenMultiplier { pub token_a_multiplier: u64, pub token_b_multiplier: u64, pub precision_factor: u8 }
    #[repr(C, packed)] #[derive(Clone, Copy, Pod, Zeroable, Debug, Default, Serialize, Deserialize)]
    pub struct PoolFees {
        pub trade_fee_numerator: u64, pub trade_fee_denominator: u64,
        pub protocol_trade_fee_numerator: u64, pub protocol_trade_fee_denominator: u64
    }
    #[repr(C, packed)] #[derive(Clone, Copy, Pod, Zeroable, Debug, Default)]
    pub struct Pool {
        pub lp_mint: Pubkey, pub token_a_mint: Pubkey, pub token_b_mint: Pubkey, pub a_vault: Pubkey,
        pub b_vault: Pubkey, pub a_vault_lp: Pubkey, pub b_vault_lp: Pubkey, pub a_vault_lp_bump: u8,
        pub enabled: u8, pub protocol_token_a_fee: Pubkey, pub protocol_token_b_fee: Pubkey,
        pub fee_last_updated_at: u64, pub _padding0: [u8; 24], pub fees: PoolFees, pub pool_type: u8,
        pub stake: Pubkey,
    }

    // --- DÉBUT DES NOUVELLES STRUCTURES POUR LE ZÉRO-COPIE ---
    #[repr(C, packed)] #[derive(Clone, Copy, Pod, Zeroable, Debug, Default, Serialize, Deserialize)]
    pub struct Depeg {
        pub base_virtual_price: u64,
        pub base_cache_updated: u64,
        pub depeg_type: u8,
        _padding: [u8; 7],
    }

    #[repr(C, packed)] #[derive(Clone, Copy, Pod, Zeroable, Debug, Default)]
    pub struct StableCurveParams {
        pub amp: u64,
        pub token_multiplier: TokenMultiplier,
        pub depeg: Depeg,
    }

    /// La "super-structure" qui représente la totalité de la disposition mémoire du compte.
    // --- MODIFICATION ICI : On retire `Pod` et `Zeroable` du `derive` car Rust ne peut pas les dériver automatiquement pour un type qui contient des `padding` non alignés
    #[repr(C, packed)]
    #[derive(Clone, Copy)] // On garde Clone et Copy
    pub struct FullPoolLayout {
        pub base_pool: Pool,
        // Ce padding est calculé pour atteindre exactement l'octet où le type de courbe est défini
        pub _padding_to_curve: [u8; 866 - size_of::<Pool>()],
        pub curve_kind: u8,
        pub stable_params: StableCurveParams,
    }

    // --- AJOUT DE CE BLOC ---
    // On dit manuellement au compilateur que notre structure est sûre.
    // C'est sans danger car elle est `repr(C, packed)` et ne contient que des types qui sont eux-mêmes `Pod`.
    unsafe impl Zeroable for FullPoolLayout {}
    unsafe impl Pod for FullPoolLayout {}
    // --- FIN DE L'AJOUT ---
}


// --- STRUCTURES DE TRAVAIL "PROPRES" ---
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MeteoraCurveType { ConstantProduct, Stable { amp: u64, token_multiplier: onchain_layouts::TokenMultiplier } }

#[repr(C, packed)]
#[derive(Clone, Copy, Pod, Zeroable, Debug, Default, Serialize, Deserialize)]
pub struct Depeg {
    pub base_virtual_price: u64,
    pub base_cache_updated: u64,
    pub depeg_type: u8, // On ne décode que le tag, pas besoin de l'enum complet
    _padding: [u8; 7],
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecodedMeteoraSbpPool {
    pub address: Pubkey, pub mint_a: Pubkey, pub mint_b: Pubkey, pub vault_a: Pubkey,
    pub vault_b: Pubkey, pub a_token_vault: Pubkey, pub b_token_vault: Pubkey, pub a_vault_lp: Pubkey,
    pub b_vault_lp: Pubkey, pub a_vault_lp_mint: Pubkey, pub b_vault_lp_mint: Pubkey,
    pub reserve_a: u64, pub reserve_b: u64, pub vault_a_state: onchain_layouts::Vault,
    pub vault_b_state: onchain_layouts::Vault, pub vault_a_lp_supply: u64, pub vault_b_lp_supply: u64,
    pub pool_a_vault_lp_amount: u64, pub pool_b_vault_lp_amount: u64, pub mint_a_decimals: u8,
    pub mint_b_decimals: u8, pub fees: onchain_layouts::PoolFees, pub curve_type: MeteoraCurveType,
    pub enabled: bool, pub stake: Pubkey, pub depeg_virtual_price: u128, pub last_swap_timestamp: i64,
}

pub fn decode_pool(address: &Pubkey, data: &[u8]) -> Result<DecodedMeteoraSbpPool> {
    if data.get(..8) != Some(&POOL_STATE_DISCRIMINATOR) {
        bail!("Invalid discriminator.");
    }

    let data_slice = &data[8..];

    // On s'assure que les données sont au moins assez longues pour la plus petite variante (ConstantProduct)
    // La taille minimale est l'offset du `curve_kind` + 1 octet pour le kind lui-même.
    const MIN_LAYOUT_SIZE: usize = 866 + 1;
    if data_slice.len() < MIN_LAYOUT_SIZE {
        bail!("Data slice too short for Meteora DAMM V1 pool.");
    }

    // 1. On fait UN SEUL appel pour créer une référence zéro-copie sur les données.
    let full_layout: &onchain_layouts::FullPoolLayout = bytemuck::from_bytes(&data_slice[..size_of::<onchain_layouts::FullPoolLayout>()]);
    let pool_struct = &full_layout.base_pool;

    // 2. On décode le type de courbe en lisant directement le champ de notre référence.
    let curve_type = match full_layout.curve_kind {
        0 => MeteoraCurveType::ConstantProduct,
        1 => {
            // On lit les paramètres stables directement depuis la référence
            MeteoraCurveType::Stable {
                amp: full_layout.stable_params.amp,
                token_multiplier: full_layout.stable_params.token_multiplier
            }
        }
        _ => bail!("Unsupported curve type: {}", full_layout.curve_kind),
    };

    // 3. On lit le prix virtuel directement depuis la référence si c'est une courbe stable.
    let depeg_virtual_price = if let MeteoraCurveType::Stable { .. } = curve_type {
        full_layout.stable_params.depeg.base_virtual_price as u128
    } else {
        0
    };

    // 4. On construit notre structure de travail propre. Le code est plus clair.
    Ok(DecodedMeteoraSbpPool {
        address: *address,
        mint_a: pool_struct.token_a_mint,
        mint_b: pool_struct.token_b_mint,
        vault_a: pool_struct.a_vault,
        vault_b: pool_struct.b_vault,
        a_vault_lp: pool_struct.a_vault_lp,
        b_vault_lp: pool_struct.b_vault_lp,
        stake: pool_struct.stake,
        enabled: pool_struct.enabled == 1,
        fees: pool_struct.fees,
        curve_type,
        depeg_virtual_price,
        // Les champs suivants sont des placeholders qui seront remplis par `hydrate`.
        a_token_vault: Pubkey::default(),
        b_token_vault: Pubkey::default(),
        a_vault_lp_mint: Pubkey::default(),
        b_vault_lp_mint: Pubkey::default(),
        reserve_a: 0,
        reserve_b: 0,
        mint_a_decimals: 0,
        mint_b_decimals: 0,
        vault_a_state: onchain_layouts::Vault::default(),
        vault_b_state: onchain_layouts::Vault::default(),
        vault_a_lp_supply: 0,
        vault_b_lp_supply: 0,
        pool_a_vault_lp_amount: 0,
        pool_b_vault_lp_amount: 0,
        last_swap_timestamp: 0,
    })
}


pub async fn hydrate(pool: &mut DecodedMeteoraSbpPool, rpc_client: &ResilientRpcClient) -> Result<()> {
    // --- Étape 1 : Premier batch de requêtes groupées (inchangé, c'est correct) ---
    let initial_keys = vec![
        pool.vault_a,
        pool.vault_b,
        pool.a_vault_lp,
        pool.b_vault_lp,
        pool.mint_a,
        pool.mint_b,
    ];

    let initial_accounts = rpc_client.get_multiple_accounts(&initial_keys).await?;
    let mut accounts_iter = initial_accounts.into_iter();

    let vault_a_account = accounts_iter.next().flatten().ok_or_else(|| anyhow!("Compte vault A ({}) non trouvé", pool.vault_a))?;
    let vault_b_account = accounts_iter.next().flatten().ok_or_else(|| anyhow!("Compte vault B ({}) non trouvé", pool.vault_b))?;
    let pool_lp_a_account_data = accounts_iter.next().flatten().ok_or_else(|| anyhow!("Compte pool LP A ({}) non trouvé", pool.a_vault_lp))?.data;
    let pool_lp_b_account_data = accounts_iter.next().flatten().ok_or_else(|| anyhow!("Compte pool LP B ({}) non trouvé", pool.b_vault_lp))?.data;
    let mint_a_data = accounts_iter.next().flatten().ok_or_else(|| anyhow!("Compte mint A ({}) non trouvé", pool.mint_a))?.data;
    let mint_b_data = accounts_iter.next().flatten().ok_or_else(|| anyhow!("Compte mint B ({}) non trouvé", pool.mint_b))?.data;

    // --- Étape 2 : Décodage partiel avec la CORRECTION de slicing ---
    let expected_vault_size = size_of::<onchain_layouts::Vault>();

    // Vérification de sécurité pour des messages d'erreur clairs
    if vault_a_account.data.len() < 8 + expected_vault_size || vault_b_account.data.len() < 8 + expected_vault_size {
        bail!("Données de compte Vault trop courtes pour le décodage.");
    }

    // *** LA CORRECTION EST ICI ***
    // On crée une tranche qui commence à l'offset 8 ET qui a la longueur exacte de la structure.
    let vault_a_data_slice = &vault_a_account.data[8..(8 + expected_vault_size)];
    let vault_a_struct: &onchain_layouts::Vault = bytemuck::from_bytes(vault_a_data_slice);
    pool.vault_a_state = *vault_a_struct;
    pool.a_token_vault = vault_a_struct.token_vault;
    pool.a_vault_lp_mint = vault_a_struct.lp_mint;

    let vault_b_data_slice = &vault_b_account.data[8..(8 + expected_vault_size)];
    let vault_b_struct: &onchain_layouts::Vault = bytemuck::from_bytes(vault_b_data_slice);
    pool.vault_b_state = *vault_b_struct;
    pool.b_token_vault = vault_b_struct.token_vault;
    pool.b_vault_lp_mint = vault_b_struct.lp_mint;
    // *** FIN DE LA CORRECTION ***

    // --- Étape 3 : Deuxième batch (inchangé, c'est correct) ---
    let secondary_keys = vec![
        pool.a_vault_lp_mint,
        pool.b_vault_lp_mint,
        solana_sdk::sysvar::clock::ID,
    ];

    let secondary_accounts = rpc_client.get_multiple_accounts(&secondary_keys).await?;
    let mut secondary_iter = secondary_accounts.into_iter();

    let vault_a_lp_mint_data = secondary_iter.next().flatten().ok_or_else(|| anyhow!("Compte LP mint A ({}) non trouvé", pool.a_vault_lp_mint))?.data;
    let vault_b_lp_mint_data = secondary_iter.next().flatten().ok_or_else(|| anyhow!("Compte LP mint B ({}) non trouvé", pool.b_vault_lp_mint))?.data;
    let clock_account_data = secondary_iter.next().flatten().ok_or_else(|| anyhow!("Compte Clock sysvar non trouvé"))?.data;

    // --- Étape 4 : Décodage final et calcul des réserves (inchangé, c'est correct) ---
    pool.mint_a_decimals = crate::decoders::spl_token_decoders::mint::decode_mint(&pool.mint_a, &mint_a_data)?.decimals;
    pool.mint_b_decimals = crate::decoders::spl_token_decoders::mint::decode_mint(&pool.mint_b, &mint_b_data)?.decimals;

    let pool_lp_a_account = SplTokenAccount::unpack(&pool_lp_a_account_data)?;
    pool.pool_a_vault_lp_amount = pool_lp_a_account.amount;

    let pool_lp_b_account = SplTokenAccount::unpack(&pool_lp_b_account_data)?;
    pool.pool_b_vault_lp_amount = pool_lp_b_account.amount;

    let vault_a_lp_mint_account = SplMint::unpack(&vault_a_lp_mint_data)?;
    pool.vault_a_lp_supply = vault_a_lp_mint_account.supply;

    let vault_b_lp_mint_account = SplMint::unpack(&vault_b_lp_mint_data)?;
    pool.vault_b_lp_supply = vault_b_lp_mint_account.supply;

    let clock: solana_sdk::sysvar::clock::Clock = bincode::deserialize(&clock_account_data)?;
    let current_timestamp = clock.unix_timestamp;

    let unlocked_a = get_unlocked_amount(&pool.vault_a_state, current_timestamp);
    let unlocked_b = get_unlocked_amount(&pool.vault_b_state, current_timestamp);
    if pool.vault_a_lp_supply > 0 {
        pool.reserve_a = get_tokens_for_lp(pool.pool_a_vault_lp_amount, unlocked_a, pool.vault_a_lp_supply);
    }
    if pool.vault_b_lp_supply > 0 {
        pool.reserve_b = get_tokens_for_lp(pool.pool_b_vault_lp_amount, unlocked_b, pool.vault_b_lp_supply);
    }

    println!("✅ Prix Virtuel lu depuis le pool state: {}", pool.depeg_virtual_price);

    Ok(())
}
// ... (fonctions get_unlocked_amount etc. inchangées)
const LOCKED_PROFIT_DEGRADATION_DENOMINATOR: u128 = 1_000_000_000_000;
fn calculate_locked_profit(tracker: &onchain_layouts::LockedProfitTracker, current_time: i64) -> u64 {
    let current_time_u64 = if current_time < 0 { 0 } else { current_time as u64 };
    let duration = u128::from(current_time_u64.saturating_sub(tracker.last_report));
    let degradation = u128::from(tracker.locked_profit_degradation);
    let ratio = duration.saturating_mul(degradation);
    if ratio > LOCKED_PROFIT_DEGRADATION_DENOMINATOR { return 0; }
    let locked_profit = u128::from(tracker.last_updated_locked_profit);
    ((locked_profit.saturating_mul(LOCKED_PROFIT_DEGRADATION_DENOMINATOR - ratio)) / LOCKED_PROFIT_DEGRADATION_DENOMINATOR) as u64
}
fn get_unlocked_amount(vault: &onchain_layouts::Vault, current_time: i64) -> u64 {
    vault.total_amount.saturating_sub(calculate_locked_profit(&vault.locked_profit_tracker, current_time))
}
fn get_tokens_for_lp(lp_amount: u64, vault_total_tokens: u64, vault_lp_supply: u64) -> u64 {
    if vault_lp_supply == 0 { return 0; }
    ((lp_amount as u128).saturating_mul(vault_total_tokens as u128) / vault_lp_supply as u128) as u64
}
fn get_lp_for_tokens(token_amount: u64, vault_total_tokens: u64, vault_lp_supply: u64) -> u64 {
    if vault_total_tokens == 0 { return token_amount; }
    ((token_amount as u128).saturating_mul(vault_lp_supply as u128) / vault_total_tokens as u128) as u64
}


#[async_trait]
impl PoolOperations for DecodedMeteoraSbpPool {
    fn get_mints(&self) -> (Pubkey, Pubkey) { (self.mint_a, self.mint_b) }
    fn get_vaults(&self) -> (Pubkey, Pubkey) { (self.vault_a, self.vault_b) }
    fn get_reserves(&self) -> (u64, u64) { (self.reserve_a, self.reserve_b) }
    fn address(&self) -> Pubkey { self.address }

    fn get_quote_with_details(&self, token_in_mint: &Pubkey, amount_in: u64, current_timestamp: i64) -> Result<QuoteResult> {

        let start_time = Instant::now();
        debug!(amount_in, "Calcul de get_quote_with_details pour Meteora DAMM V1");

        if !self.enabled || amount_in == 0 {
            return Ok(QuoteResult::default()); // Retourne un résultat vide
        }


        let (in_reserve, out_reserve, in_vault_state, out_vault_state, in_vault_lp_supply, out_vault_lp_supply, in_pool_lp_amount, _out_pool_lp_amount) =
            if *token_in_mint == self.mint_a {
                (self.reserve_a, self.reserve_b, self.vault_a_state, self.vault_b_state, self.vault_a_lp_supply, self.vault_b_lp_supply, self.pool_a_vault_lp_amount, self.pool_b_vault_lp_amount)
            } else {
                (self.reserve_b, self.reserve_a, self.vault_b_state, self.vault_a_state, self.vault_b_lp_supply, self.vault_a_lp_supply, self.pool_b_vault_lp_amount, self.pool_a_vault_lp_amount)
            };

        let fees = &self.fees;
        // **CORRECTION PRECISION 1 : Utilisation de div_ceil pour les frais**
        let trade_fee_on_input = (amount_in as u128).saturating_mul(fees.trade_fee_numerator as u128).div_ceil(fees.trade_fee_denominator as u128);
        let protocol_fee = trade_fee_on_input.saturating_mul(fees.protocol_trade_fee_numerator as u128).div_ceil(fees.protocol_trade_fee_denominator as u128);
        let amount_in_after_protocol_fee = amount_in.saturating_sub(protocol_fee as u64);

        let unlocked_in_vault_before = get_unlocked_amount(&in_vault_state, current_timestamp);
        let in_lp_minted = get_lp_for_tokens(amount_in_after_protocol_fee, unlocked_in_vault_before, in_vault_lp_supply);

        let mut temp_in_vault_state = in_vault_state;
        temp_in_vault_state.total_amount = in_vault_state.total_amount.saturating_add(amount_in_after_protocol_fee);
        let unlocked_in_vault_after = get_unlocked_amount(&temp_in_vault_state, current_timestamp);
        let new_in_vault_lp_supply = in_vault_lp_supply.saturating_add(in_lp_minted);
        let new_in_pool_lp_amount = in_pool_lp_amount.saturating_add(in_lp_minted);
        let new_in_reserve = get_tokens_for_lp(new_in_pool_lp_amount, unlocked_in_vault_after, new_in_vault_lp_supply);
        let actual_amount_in = new_in_reserve.saturating_sub(in_reserve);

        // **CORRECTION PRECISION 2 : Utilisation de div_ceil pour les frais**
        let trade_fee_on_actual = (actual_amount_in as u128).saturating_mul(fees.trade_fee_numerator as u128).div_ceil(fees.trade_fee_denominator as u128);
        let lp_fee = trade_fee_on_actual.saturating_sub(protocol_fee);
        let net_amount_in_for_curve = actual_amount_in.saturating_sub(lp_fee as u64);

        let amount_out_gross = match &self.curve_type {
            MeteoraCurveType::ConstantProduct => {
                let numerator = (net_amount_in_for_curve as u128).saturating_mul(out_reserve as u128);
                let denominator = (in_reserve as u128).saturating_add(net_amount_in_for_curve as u128);
                let result_value = if denominator == 0 { 0 } else { (numerator / denominator) as u64 };
                Ok(result_value) as Result<u64>
            }
            MeteoraCurveType::Stable { amp, token_multiplier } => {
                const DEPEG_PRECISION: u128 = 1_000_000;
                let virtual_price = if self.depeg_virtual_price > 0 { self.depeg_virtual_price } else { DEPEG_PRECISION };

                let (scaled_in_reserve, scaled_out_reserve, scaled_in_amount, final_downscale_divisor) =
                    if self.stake != Pubkey::default() {
                        if *token_in_mint == self.mint_a {
                            ( (in_reserve as u128).saturating_mul(DEPEG_PRECISION), (out_reserve as u128).saturating_mul(virtual_price), (net_amount_in_for_curve as u128).saturating_mul(DEPEG_PRECISION), virtual_price )
                        } else {
                            ( (in_reserve as u128).saturating_mul(virtual_price), (out_reserve as u128).saturating_mul(DEPEG_PRECISION), (net_amount_in_for_curve as u128).saturating_mul(virtual_price), DEPEG_PRECISION )
                        }
                    } else {
                        (in_reserve as u128, out_reserve as u128, net_amount_in_for_curve as u128, 1)
                    };

                let (in_mult, out_mult) = if *token_in_mint == self.mint_a { (token_multiplier.token_a_multiplier, token_multiplier.token_b_multiplier) } else { (token_multiplier.token_b_multiplier, token_multiplier.token_a_multiplier) };
                let norm_in_reserve = scaled_in_reserve.saturating_mul(in_mult as u128);
                let norm_out_reserve = scaled_out_reserve.saturating_mul(out_mult as u128);
                let norm_in_amount = scaled_in_amount.saturating_mul(in_mult as u128);

                let norm_out = math::get_quote(norm_in_amount, norm_in_reserve, norm_out_reserve, *amp)?;

                let pre_depeg_out = norm_out.div_ceil(out_mult as u128);
                let final_out = pre_depeg_out.checked_div(final_downscale_divisor).unwrap_or(0);

                Ok(final_out as u64)
            }
        }?;

        let unlocked_out_vault = get_unlocked_amount(&out_vault_state, current_timestamp);
        let out_lp_to_burn = get_lp_for_tokens(amount_out_gross, unlocked_out_vault, out_vault_lp_supply);
        let final_amount_out = get_tokens_for_lp(out_lp_to_burn, unlocked_out_vault, out_vault_lp_supply);

        metrics::GET_QUOTE_LATENCY.with_label_values(&["MeteoraDammV1"]).observe(start_time.elapsed().as_secs_f64());


        Ok(QuoteResult {
            amount_out: final_amount_out,
            fee: lp_fee as u64, // Estimation du LP fee
            ticks_crossed: 0,   // Pas de ticks pour ce type de pool
        })
    }

    fn get_required_input(&mut self, token_out_mint: &Pubkey, amount_out: u64, current_timestamp: i64) -> Result<u64> {
        let start_time = Instant::now();
        debug!(amount_out, "Calcul de get_required_input pour Meteora DAMM V1");

        if amount_out == 0 { return Ok(0); }
        if !self.enabled { return Err(anyhow!("Pool disabled")); }

        let token_in_mint = if *token_out_mint == self.mint_a { self.mint_b } else { self.mint_a };

        let (in_reserve, out_reserve) = if *token_out_mint == self.mint_b {
            (self.reserve_a, self.reserve_b)
        } else {
            (self.reserve_b, self.reserve_a)
        };

        // --- HEURISTIQUE DE BORNE SUPÉRIEURE SIMPLE ET LARGE ---
        let mut high_bound: u64;
        if out_reserve > amount_out {
            // Estimation de base très grossière
            let base_estimate = (amount_out as u128 * in_reserve as u128 / (out_reserve - amount_out) as u128) as u64;
            // On prend une marge de sécurité très large (ex: 4x) pour être absolument sûr de couvrir tous les frais et effets de vault.
            high_bound = base_estimate.saturating_mul(4);
        } else {
            // Si on demande plus que la réserve, l'input est potentiellement énorme.
            // On prend une borne très élevée mais pas u64::MAX pour éviter les overflows.
            high_bound = in_reserve.saturating_mul(10);
        }

        // Sécurité supplémentaire : si la borne est toujours 0, on met une valeur par défaut.
        if high_bound == 0 {
            high_bound = amount_out.saturating_mul(10);
        }


        let quote_fn = |guess_input: u64| {
            self.get_quote(&token_in_mint, guess_input, current_timestamp)
        };

        let result = find_input_by_binary_search(quote_fn, amount_out, high_bound);

        // --- AJOUT JUSTE AVANT DE RETOURNER ---
        metrics::GET_QUOTE_LATENCY.with_label_values(&["MeteoraDammV1_required_input"]).observe(start_time.elapsed().as_secs_f64());

        result
    }
    fn update_from_account_data(&mut self, account_pubkey: &Pubkey, account_data: &[u8]) -> Result<()> {
        // Les données d'un compte token SPL ont le solde (u64) à l'offset 64.
        if account_data.len() >= 72 {
            let balance = u64::from_le_bytes(account_data[64..72].try_into()?);

            // On vérifie si le compte mis à jour est un des vaults que l'on connaît
            if *account_pubkey == self.a_token_vault {
                self.reserve_a = balance;
            } else if *account_pubkey == self.b_token_vault {
                self.reserve_b = balance;
            }
        }
        Ok(())
    }

    fn create_swap_instruction(&self, token_in_mint: &Pubkey, amount_in: u64, min_amount_out: u64, user_accounts: &UserSwapAccounts) -> Result<Instruction> {
        let instruction_discriminator: [u8; 8] = [248, 198, 158, 145, 225, 117, 135, 200];
        let mut instruction_data = Vec::with_capacity(8 + 8 + 8);
        instruction_data.extend_from_slice(&instruction_discriminator);
        instruction_data.extend_from_slice(&amount_in.to_le_bytes());
        instruction_data.extend_from_slice(&min_amount_out.to_le_bytes());

        // **CORRECTION ICI : On dérive le PDA du compte de frais dynamiquement, comme le fait le SDK.**
        // C'est la méthode la plus robuste.
        let (protocol_fee_account, _) = Pubkey::find_program_address(
            &[
                b"fee",
                token_in_mint.as_ref(),
                self.address.as_ref()
            ],
            &PROGRAM_ID
        );

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
            AccountMeta::new(protocol_fee_account, false), // Utilisation du PDA dérivé
            AccountMeta::new_readonly(user_accounts.owner, true),
            AccountMeta::new_readonly(VAULT_PROGRAM_ID, false),
            AccountMeta::new_readonly(spl_token::id(), false),
        ];

        // On ajoute le compte de stake UNIQUEMENT si nécessaire.
        if let MeteoraCurveType::Stable {..} = self.curve_type {
            if self.stake != Pubkey::default() {
                accounts.push(AccountMeta::new_readonly(self.stake, false));
            }
        }

        Ok(Instruction { program_id: PROGRAM_ID, accounts, data: instruction_data })
    }
}