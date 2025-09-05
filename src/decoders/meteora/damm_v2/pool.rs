// DANS: src/decoders/meteora/damm_v2/pool.rs

use crate::decoders::spl_token_decoders;
use anyhow::{anyhow, bail, Result};
use bytemuck::{Pod, Zeroable};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::pubkey::Pubkey;
use uint::construct_uint;
use solana_sdk::instruction::{Instruction, AccountMeta};
use spl_token;
use serde::{Serialize, Deserialize};
use async_trait::async_trait;
use crate::decoders::pool_operations::{PoolOperations, UserSwapAccounts};
use super::math;
use crate::decoders::pool_operations::find_input_by_binary_search;

construct_uint! { pub struct U256(4); }

pub const PROGRAM_ID: Pubkey = solana_sdk::pubkey!("cpamdpZCGKUy5JxQXB4dcpGPiikHawvSWAd6mEn1sGG");
pub const POOL_STATE_DISCRIMINATOR: [u8; 8] = [241, 154, 109, 4, 17, 177, 109, 188];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecodedMeteoraDammPool {
    pub address: Pubkey,
    pub mint_a: Pubkey,
    pub mint_b: Pubkey,
    pub vault_a: Pubkey,
    pub vault_b: Pubkey,
    pub liquidity: u128,
    pub sqrt_price: u128,
    pub sqrt_min_price: u128,
    pub sqrt_max_price: u128,
    pub collect_fee_mode: u8,
    pub mint_a_decimals: u8,
    pub mint_b_decimals: u8,
    pub mint_a_transfer_fee_bps: u16,
    pub mint_b_transfer_fee_bps: u16,
    pub mint_a_program: Pubkey,
    pub mint_b_program: Pubkey,
    pub pool_fees: onchain_layouts::PoolFeesStruct,
    pub activation_point: u64,
    pub last_swap_timestamp: i64,
}

impl DecodedMeteoraDammPool {
    pub fn fee_as_percent(&self) -> f64 {
        let base_fee = self.pool_fees.base_fee.cliff_fee_numerator;
        if base_fee == 0 { return 0.0; }
        (base_fee as f64 / 1_000_000_000.0) * 100.0
    }
}

pub mod onchain_layouts {
    #![allow(dead_code)]
    use super::*;
    #[repr(C, packed)] #[derive(Copy, Clone, Pod, Zeroable, Debug)]
    pub struct Pool { pub pool_fees: PoolFeesStruct, pub token_a_mint: Pubkey, pub token_b_mint: Pubkey, pub token_a_vault: Pubkey, pub token_b_vault: Pubkey, pub whitelisted_vault: Pubkey, pub partner: Pubkey, pub liquidity: u128, pub _padding: u128, pub protocol_a_fee: u64, pub protocol_b_fee: u64, pub partner_a_fee: u64, pub partner_b_fee: u64, pub sqrt_min_price: u128, pub sqrt_max_price: u128, pub sqrt_price: u128, pub activation_point: u64, pub activation_type: u8, pub pool_status: u8, pub token_a_flag: u8, pub token_b_flag: u8, pub collect_fee_mode: u8, pub pool_type: u8, pub _padding_0: [u8; 2], pub fee_a_per_liquidity: [u8; 32], pub fee_b_per_liquidity: [u8; 32], pub permanent_lock_liquidity: u128, pub metrics: PoolMetrics, pub creator: Pubkey, pub _padding_1: [u64; 6], pub reward_infos: [RewardInfo; 2], }
    #[repr(C, packed)] #[derive(Copy, Clone, Pod, Zeroable, Debug, Serialize, Deserialize)]
    pub struct PoolFeesStruct { pub base_fee: BaseFeeStruct, pub protocol_fee_percent: u8, pub partner_fee_percent: u8, pub referral_fee_percent: u8, pub padding_0: [u8; 5], pub dynamic_fee: DynamicFeeStruct, pub padding_1: [u64; 2], }
    #[repr(C, packed)] #[derive(Copy, Clone, Pod, Zeroable, Debug, Serialize, Deserialize)]
    pub struct BaseFeeStruct { pub cliff_fee_numerator: u64, pub fee_scheduler_mode: u8, pub padding_0: [u8; 5], pub number_of_period: u16, pub period_frequency: u64, pub reduction_factor: u64, pub padding_1: u64, }
    #[repr(C, packed)] #[derive(Copy, Clone, Pod, Zeroable, Debug, Serialize, Deserialize)]
    pub struct DynamicFeeStruct { pub initialized: u8, pub padding: [u8; 7], pub max_volatility_accumulator: u32, pub variable_fee_control: u32, pub bin_step: u16, pub filter_period: u16, pub decay_period: u16, pub reduction_factor: u16, pub last_update_timestamp: u64, pub bin_step_u128: u128, pub sqrt_price_reference: u128, pub volatility_accumulator: u128, pub volatility_reference: u128, }
    #[repr(C, packed)] #[derive(Copy, Clone, Pod, Zeroable, Debug)]
    pub struct PoolMetrics { pub total_lp_a_fee: u128, pub total_lp_b_fee: u128, pub total_protocol_a_fee: u64, pub total_protocol_b_fee: u64, pub total_partner_a_fee: u64, pub total_partner_b_fee: u64, pub total_position: u64, pub padding: u64, }
    #[repr(C, packed)] #[derive(Copy, Clone, Pod, Zeroable, Debug)]
    pub struct RewardInfo { pub initialized: u8, pub reward_token_flag: u8, pub _padding_0: [u8; 6], pub _padding_1: [u8; 8], pub mint: Pubkey, pub vault: Pubkey, pub funder: Pubkey, pub reward_duration: u64, pub reward_duration_end: u64, pub reward_rate: u128, pub reward_per_token_stored: [u8; 32], pub last_update_time: u64, pub cumulative_seconds_with_empty_liquidity_reward: u64, }
}

pub fn decode_pool(address: &Pubkey, data: &[u8]) -> Result<DecodedMeteoraDammPool> {
    if data.get(..8) != Some(&POOL_STATE_DISCRIMINATOR) {
        bail!("Invalid discriminator. Not a Meteora DAMM v2 Pool account.");
    }
    let data_slice = &data[8..];
    let expected_size = size_of::<onchain_layouts::Pool>();
    if data_slice.len() < expected_size {
        bail!("DAMM v2 Pool data length mismatch. Expected at least {} bytes, got {}.", expected_size, data_slice.len());
    }
    let pool_struct: onchain_layouts::Pool = bytemuck::pod_read_unaligned(&data_slice[..expected_size]);

    Ok(DecodedMeteoraDammPool {
        address: *address,
        mint_a: pool_struct.token_a_mint,
        mint_b: pool_struct.token_b_mint,
        vault_a: pool_struct.token_a_vault,
        vault_b: pool_struct.token_b_vault,
        liquidity: pool_struct.liquidity,
        sqrt_price: pool_struct.sqrt_price,
        sqrt_min_price: pool_struct.sqrt_min_price,
        sqrt_max_price: pool_struct.sqrt_max_price,
        collect_fee_mode: pool_struct.collect_fee_mode,
        pool_fees: pool_struct.pool_fees,
        activation_point: pool_struct.activation_point,
        mint_a_decimals: 0, mint_b_decimals: 0,
        mint_a_transfer_fee_bps: 0, mint_b_transfer_fee_bps: 0,
        mint_a_program: spl_token::id(),
        mint_b_program: spl_token::id(),
        last_swap_timestamp: 0,
    })
}

pub async fn hydrate(pool: &mut DecodedMeteoraDammPool, rpc_client: &RpcClient) -> Result<()> {
    // 1. Rassembler les clés publiques des deux mints.
    let mint_keys = vec![pool.mint_a, pool.mint_b];

    // 2. Faire un seul appel RPC pour récupérer les deux comptes.
    let accounts_data = rpc_client.get_multiple_accounts(&mint_keys).await?;

    // 3. Traiter les résultats avec une gestion d'erreur robuste.
    let mut accounts_iter = accounts_data.into_iter();

    let mint_a_account = accounts_iter.next().flatten()
        .ok_or_else(|| anyhow!("Compte mint A ({}) non trouvé", pool.mint_a))?;
    let mint_b_account = accounts_iter.next().flatten()
        .ok_or_else(|| anyhow!("Compte mint B ({}) non trouvé", pool.mint_b))?;

    // 4. Décoder les données et mettre à jour le pool.
    let decoded_mint_a = spl_token_decoders::mint::decode_mint(&pool.mint_a, &mint_a_account.data)?;
    pool.mint_a_decimals = decoded_mint_a.decimals;
    pool.mint_a_transfer_fee_bps = decoded_mint_a.transfer_fee_basis_points;
    pool.mint_a_program = mint_a_account.owner;

    let decoded_mint_b = spl_token_decoders::mint::decode_mint(&pool.mint_b, &mint_b_account.data)?;
    pool.mint_b_decimals = decoded_mint_b.decimals;
    pool.mint_b_transfer_fee_bps = decoded_mint_b.transfer_fee_basis_points;
    pool.mint_b_program = mint_b_account.owner;

    Ok(())
}
#[async_trait]
impl PoolOperations for DecodedMeteoraDammPool {
    fn get_mints(&self) -> (Pubkey, Pubkey) { (self.mint_a, self.mint_b) }
    fn get_vaults(&self) -> (Pubkey, Pubkey) { (self.vault_a, self.vault_b) }

    fn get_reserves(&self) -> (u64, u64) {
        // Pour DAMM V2, on peut faire une estimation des réserves effectives
        // en utilisant la liquidité et le prix actuels.
        let price_x64 = U256::from(self.sqrt_price);
        let liquidity_u256 = U256::from(self.liquidity);
        let q64 = U256::one() << 64;

        if price_x64.is_zero() { return (0, 0); }

        // Formules inversées : amount_a = L / sqrt(P), amount_b = L * sqrt(P)
        // C'est une approximation de la liquidité "autour" du prix actuel.
        let estimated_reserve_a = (liquidity_u256 * q64 / price_x64).as_u64();
        let estimated_reserve_b = ((liquidity_u256 * price_x64) >> 64).as_u64();

        (estimated_reserve_a, estimated_reserve_b)
    }

    fn address(&self) -> Pubkey { self.address }

    fn get_quote(&self, token_in_mint: &Pubkey, amount_in: u64, current_timestamp: i64) -> Result<u64> {
        if self.liquidity == 0 { return Ok(0); }
        let a_to_b = *token_in_mint == self.mint_a;
        let (in_mint_fee_bps, out_mint_fee_bps) = if a_to_b {
            (self.mint_a_transfer_fee_bps, self.mint_b_transfer_fee_bps)
        } else {
            (self.mint_b_transfer_fee_bps, self.mint_a_transfer_fee_bps)
        };
        let fee_on_input = (amount_in as u128 * in_mint_fee_bps as u128) / 10000;
        let amount_in_after_transfer_fee = amount_in.saturating_sub(fee_on_input as u64);
        let fees = &self.pool_fees;
        let base_fee_num = math::get_base_fee(&fees.base_fee, current_timestamp, self.activation_point)? as u128;
        let dynamic_fee_num = if fees.dynamic_fee.initialized != 0 {
            math::get_variable_fee(&fees.dynamic_fee)?
        } else { 0 };
        let total_fee_numerator = base_fee_num.saturating_add(dynamic_fee_num);
        const FEE_DENOMINATOR: u128 = 1_000_000_000;
        let fees_on_input = match (self.collect_fee_mode, a_to_b) {
            (0, _) => false,
            (1, true) => false,
            (1, false) => true,
            _ => bail!("Unsupported collect_fee_mode"),
        };
        let (pre_fee_amount_out, total_fee_amount) = if fees_on_input {
            let total_fee = (amount_in_after_transfer_fee as u128 * total_fee_numerator) / FEE_DENOMINATOR;
            let net_in = amount_in_after_transfer_fee.saturating_sub(total_fee as u64);
            let next_sqrt_price = math::get_next_sqrt_price_from_input(self.sqrt_price, self.liquidity, net_in, a_to_b)?;
            let out = math::get_amount_out(self.sqrt_price, next_sqrt_price, self.liquidity, a_to_b)?;
            (out, total_fee)
        } else {
            let next_sqrt_price = math::get_next_sqrt_price_from_input(self.sqrt_price, self.liquidity, amount_in_after_transfer_fee, a_to_b)?;
            let out = math::get_amount_out(self.sqrt_price, next_sqrt_price, self.liquidity, a_to_b)?;
            let total_fee = (out as u128 * total_fee_numerator) / FEE_DENOMINATOR;
            (out, total_fee)
        };
        let protocol_fee_percent = fees.protocol_fee_percent as u128;
        let partner_fee_percent = fees.partner_fee_percent as u128;
        let protocol_fee = (total_fee_amount * protocol_fee_percent) / 100;
        let partner_fee = (total_fee_amount * partner_fee_percent) / 100;
        let lp_fee = total_fee_amount.saturating_sub(protocol_fee).saturating_sub(partner_fee);
        let fees_to_deduct = lp_fee.saturating_add(protocol_fee).saturating_add(partner_fee);
        let amount_out_after_pool_fee = if !fees_on_input {
            pre_fee_amount_out.saturating_sub(fees_to_deduct as u64)
        } else {
            pre_fee_amount_out
        };
        let fee_on_output = (amount_out_after_pool_fee as u128 * out_mint_fee_bps as u128) / 10000;
        let final_amount_out = amount_out_after_pool_fee.saturating_sub(fee_on_output as u64);
        Ok(final_amount_out)
    }

    /// Calcule le montant d'entrée requis pour obtenir un montant de sortie spécifié.
    ///
    /// # Stratégie d'Implémentation : Recherche Binaire (Dichotomie)
    ///
    /// L'inversion mathématique directe (analytique) de la fonction `get_quote` pour ce pool
    /// est extrêmement instable. La combinaison de la liquidité concentrée (CLMM), des frais de transfert
    /// Token-2022, et surtout des frais de pool conditionnels (parfois sur l'input, parfois sur l'output)
    /// rend une formule d'inversion directe très sensible aux erreurs d'arrondi, menant à des résultats incorrects.
    /// Le SDK de Meteora n'expose d'ailleurs pas cette fonction, ce qui confirme sa complexité.
    ///
    /// Pour garantir la robustesse et la précision, nous utilisons une méthode numérique : la recherche binaire.
    /// L'algorithme cherche le plus petit `amount_in` qui produit un `quote_output >= amount_out`.
    ///
    /// # Choix de la Boucle : Dynamique vs. Fixe (Déterministe)
    ///
    /// Il existe deux approches pour la boucle de recherche :
    ///
    /// 1.  **Approche Fixe (Déterministe) :** `for _ in 0..32`.
    ///     - **Avantages :** Temps d'exécution prévisible. Le calcul prend toujours le même temps,
    ///       permettant au bot de faire des "méta-décisions" (ex: "Mon temps de calcul est de 12ms,
    ///       la fenêtre d'opportunité est de 50ms, je sais à l'avance si j'ai le temps"). C'est crucial
    ///       pour les systèmes à très faible latence afin de rejeter instantanément les opportunités non viables.
    ///     - **Inconvénients :** Peut effectuer des itérations "inutiles" si la solution est trouvée rapidement.
    ///
    /// 2.  **Approche Dynamique (Opportuniste) - CELLE CHOISIE ICI :** `while lower <= upper`.
    ///     - **Avantages :** Potentiellement plus rapide en moyenne, car la boucle s'arrête dès que la
    ///       meilleure solution est trouvée. Vise la vitesse maximale pour chaque calcul individuel.
    ///     - **Inconvénients :** Temps d'exécution non déterministe ("jitter"). Le bot ne peut connaître
    ///       le temps de calcul qu'après l'avoir terminé, ce qui l'empêche de rejeter une opportunité
    ///       à l'avance sur la base du temps. Il peut gaspiller des ressources CPU sur des courses déjà perdues.
    ///
    /// Cette implémentation utilise l'approche dynamique pour privilégier la vitesse moyenne,
    /// tout en incluant une limite maximale d'itérations comme filet de sécurité contre les boucles infinies.
    fn get_required_input(
        &mut self,
        token_out_mint: &Pubkey,
        amount_out: u64,
        current_timestamp: i64,
    ) -> Result<u64> {
        // 1. Logique spécifique au pool
        if amount_out == 0 { return Ok(0); }
        if self.liquidity == 0 { return Err(anyhow!("Pool has no liquidity")); }

        let token_in_mint = if *token_out_mint == self.mint_a { self.mint_b } else { self.mint_a };

        // 2. Calcul de la borne de recherche
        let mut high_bound: u64 = amount_out.saturating_mul(4); // Estimation large et sûre
        if self.sqrt_price > 0 {
            // Utilise f64 pour une estimation rapide, ce n'est pas pour le calcul final.
            let price_ratio = (self.sqrt_price as f64 / (1u128 << 64) as f64).powi(2);
            let estimated_price = if token_in_mint == self.mint_a { 1.0 / price_ratio } else { price_ratio };
            // Multiplie par 1.2 pour avoir une marge de sécurité sur les frais.
            let initial_guess = (amount_out as f64 * estimated_price * 1.2) as u64;
            if initial_guess > 0 {
                high_bound = initial_guess;
            }
        }

        // 3. Création de la closure
        let quote_fn = |guess_input: u64| {
            self.get_quote(&token_in_mint, guess_input, current_timestamp)
        };

        // 4. Appel à l'utilitaire de recherche dynamique
        find_input_by_binary_search(quote_fn, amount_out, high_bound)
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

        let (pool_authority, _) = Pubkey::find_program_address(&[b"pool_authority"], &PROGRAM_ID);
        let (event_authority, _) = Pubkey::find_program_address(&[b"__event_authority"], &PROGRAM_ID);

        let (input_token_program, output_token_program) = if *token_in_mint == self.mint_a {
            (self.mint_a_program, self.mint_b_program)
        } else {
            (self.mint_b_program, self.mint_a_program)
        };

        let accounts = vec![
            AccountMeta::new_readonly(pool_authority, false),
            AccountMeta::new(self.address, false),
            AccountMeta::new(user_accounts.source, false),
            AccountMeta::new(user_accounts.destination, false),
            AccountMeta::new(self.vault_a, false),
            AccountMeta::new(self.vault_b, false),
            AccountMeta::new_readonly(self.mint_a, false),
            AccountMeta::new_readonly(self.mint_b, false),
            AccountMeta::new_readonly(user_accounts.owner, true),
            AccountMeta::new_readonly(input_token_program, false),
            AccountMeta::new_readonly(output_token_program, false),
            AccountMeta::new_readonly(PROGRAM_ID, false), // referral_token_account
            AccountMeta::new_readonly(event_authority, false),
            AccountMeta::new_readonly(PROGRAM_ID, false),
        ];

        Ok(Instruction {
            program_id: PROGRAM_ID,
            accounts,
            data: instruction_data,
        })
    }
}