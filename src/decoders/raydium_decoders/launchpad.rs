// src/decoders/raydium_decoders/launchpad.rs

use crate::decoders::pool_operations::PoolOperations;
use bytemuck::{from_bytes, Pod, Zeroable};
use solana_sdk::pubkey::Pubkey;
use anyhow::{bail, Result, anyhow};
use solana_client::nonblocking::rpc_client::RpcClient;
use crate::decoders::raydium_decoders::global_config;
use crate::math::launchpad_math;
use crate::decoders::spl_token_decoders;

// --- STRUCTURES PUBLIQUES ---

/// Énumère les types de courbes de prix possibles pour un pool Launchpad.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CurveType {
    ConstantProduct,
    FixedPrice,
    Linear,
    Unknown,
}


/// Contient toutes les informations, y compris le type de courbe, pour un pool Launchpad.
#[derive(Debug, Clone)]
pub struct DecodedLaunchpadPool {
    pub address: Pubkey,
    pub mint_a: Pubkey,
    pub mint_b: Pubkey,
    pub vault_a: Pubkey,
    pub vault_b: Pubkey,
    pub global_config: Pubkey,
    pub virtual_base: u64,
    pub virtual_quote: u64,
    pub trade_fee_rate: u64,
    pub mint_a_transfer_fee_bps: u16,
    pub mint_b_transfer_fee_bps: u16,
    pub reserve_a: u64,
    pub reserve_b: u64,
    // --- CHAMP AJOUTÉ POUR LA COURBE LINÉAIRE ---
    pub total_base_sell: u64,
    pub curve_type: CurveType, // Le champ clé pour la logique polymorphe
}

// Dans launchpad.rs, après la struct DecodedLaunchpadPool
impl DecodedLaunchpadPool {
    /// Calcule et retourne les frais de pool sous forme de pourcentage lisible.
    pub fn fee_as_percent(&self) -> f64 {
        // total_fee_percent est un ratio
        (self.trade_fee_rate as f64 / 1_000_000.0) * 100.0
    }
}

// --- STRUCTURES BRUTES (ne changent pas) ---
const POOL_STATE_DISCRIMINATOR: [u8; 8] = [247, 237, 227, 245, 215, 195, 222, 70];

#[repr(C, packed)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
struct VestingSchedule {
    pub total_locked_amount: u64, pub cliff_period: u64,
    pub unlock_period: u64, pub start_time: u64,
    pub allocated_share_amount: u64,
}

#[repr(C, packed)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
struct PoolStateData {
    pub epoch: u64, pub auth_bump: u8, pub status: u8,
    pub base_decimals: u8, pub quote_decimals: u8, pub migrate_type: u8,
    pub supply: u64, pub total_base_sell: u64, pub virtual_base: u64,
    pub virtual_quote: u64, pub real_base: u64, pub real_quote: u64,
    pub total_quote_fund_raising: u64, pub quote_protocol_fee: u64,
    pub platform_fee: u64, pub migrate_fee: u64,
    pub vesting_schedule: VestingSchedule, pub global_config: Pubkey,
    pub platform_config: Pubkey, pub base_mint: Pubkey, pub quote_mint: Pubkey,
    pub base_vault: Pubkey, pub quote_vault: Pubkey, pub creator: Pubkey,
    pub padding_for_future: [u64; 8],
}

/// Décode un compte Raydium Launchpad PoolState.
pub fn decode_pool(address: &Pubkey, data: &[u8]) -> Result<DecodedLaunchpadPool> {
    if data.get(..8) != Some(&POOL_STATE_DISCRIMINATOR) {
        bail!("Invalid discriminator.");
    }
    let data_slice = &data[8..];
    if data_slice.len() != std::mem::size_of::<PoolStateData>() {
        bail!("Data length mismatch.");
    }
    let pool_struct: &PoolStateData = from_bytes(data_slice);

    Ok(DecodedLaunchpadPool {
        address: *address,
        mint_a: pool_struct.base_mint,
        mint_b: pool_struct.quote_mint,
        vault_a: pool_struct.base_vault,
        vault_b: pool_struct.quote_vault,
        global_config: pool_struct.global_config,
        virtual_base: pool_struct.virtual_base,
        virtual_quote: pool_struct.virtual_quote,
        trade_fee_rate: 0,
        reserve_a: pool_struct.real_base,
        reserve_b: pool_struct.real_quote,
        // --- NOUVEAU CHAMP INITIALISÉ ---
        total_base_sell: pool_struct.total_base_sell,
        mint_a_transfer_fee_bps: 0,
        mint_b_transfer_fee_bps: 0,

        curve_type: CurveType::Unknown, // Sera hydraté par le graph_engine
    })
}

// --- LOGIQUE DE POOL ---
impl PoolOperations for DecodedLaunchpadPool {
    fn get_mints(&self) -> (Pubkey, Pubkey) { (self.mint_a, self.mint_b) }
    fn get_vaults(&self) -> (Pubkey, Pubkey) { (self.vault_a, self.vault_b) }

    fn get_quote(&self, token_in_mint: &Pubkey, amount_in: u64) -> Result<u64> {
        let is_buy = *token_in_mint == self.mint_b; // True si on achète A (base) avec B (quote)

        // --- 1. Appliquer les frais de transfert sur l'INPUT ---
        let (in_mint_fee_bps, out_mint_fee_bps) = if is_buy { // input = mint_b (quote), output = mint_a (base)
            (self.mint_b_transfer_fee_bps, self.mint_a_transfer_fee_bps)
        } else { // input = mint_a (base), output = mint_b (quote)
            (self.mint_a_transfer_fee_bps, self.mint_b_transfer_fee_bps)
        };

        let fee_on_input = (amount_in as u128 * in_mint_fee_bps as u128) / 10000;
        let amount_in_after_transfer_fee = amount_in.saturating_sub(fee_on_input as u64);

        // --- 2. Calculer le swap BRUT avec le montant NET ---
        let gross_amount_out = match self.curve_type {
            CurveType::ConstantProduct => {
                let (in_reserve, out_reserve) = if is_buy {
                    (self.virtual_quote, self.virtual_base)
                } else {
                    (self.virtual_base, self.virtual_quote)
                };
                if in_reserve == 0 { return Ok(0); }

                let numerator = (amount_in_after_transfer_fee as u128).saturating_mul(out_reserve as u128);
                let denominator = (in_reserve as u128).saturating_add(amount_in_after_transfer_fee as u128);
                if denominator == 0 { return Ok(0); }
                numerator.saturating_div(denominator) as u64
            },
            CurveType::FixedPrice => {
                if is_buy {
                    if self.virtual_quote == 0 { return Ok(0); }
                    let numerator = (amount_in_after_transfer_fee as u128).saturating_mul(self.virtual_base as u128);
                    let denominator = self.virtual_quote as u128;
                    numerator.saturating_div(denominator) as u64
                } else {
                    if self.virtual_base == 0 { return Ok(0); }
                    let numerator = (amount_in_after_transfer_fee as u128).saturating_mul(self.virtual_quote as u128);
                    let denominator = self.virtual_base as u128;
                    numerator.saturating_div(denominator) as u64
                }
            },
            CurveType::Linear => {
                launchpad_math::get_quote_linear_curve(
                    amount_in_after_transfer_fee,
                    self.total_base_sell,
                    self.virtual_base,
                    self.virtual_quote,
                    is_buy,
                )?
            },
            CurveType::Unknown => {
                return Err(anyhow!("Launchpad pool curve type is unknown. Hydrate first."));
            }
        };

        // --- 3. Appliquer les frais de POOL ---
        const FEE_PRECISION: u128 = 1_000_000;
        let amount_out_after_pool_fee = (gross_amount_out as u128)
            .saturating_mul(FEE_PRECISION.saturating_sub(self.trade_fee_rate as u128))
            / FEE_PRECISION;

        // --- 4. Appliquer les frais de transfert sur l'OUTPUT ---
        let fee_on_output = (amount_out_after_pool_fee * out_mint_fee_bps as u128) / 10000;
        let final_amount_out = (amount_out_after_pool_fee).saturating_sub(fee_on_output);

        Ok(final_amount_out as u64)
    }
}

pub async fn hydrate(pool: &mut DecodedLaunchpadPool, rpc_client: &RpcClient) -> Result<()> {
    // --- ON LANCE TOUS LES APPELS EN PARALLÈLE ---
    let (config_res, mint_a_res, mint_b_res) = tokio::join!(
        rpc_client.get_account_data(&pool.global_config),
        rpc_client.get_account_data(&pool.mint_a),
        rpc_client.get_account_data(&pool.mint_b)
    );

    // --- Traitement de la config ---
    let config_data = config_res?;
    let config = global_config::decode_global_config(&config_data)?;
    pool.trade_fee_rate = config.trade_fee_rate;
    pool.curve_type = match config.curve_type {
        0 => CurveType::ConstantProduct,
        1 => CurveType::FixedPrice,
        2 => CurveType::Linear,
        _ => CurveType::Unknown,
    };

    // --- AJOUT DE LA LOGIQUE D'HYDRATATION DES MINTS ---
    let mint_a_data = mint_a_res?;
    let decoded_mint_a = spl_token_decoders::mint::decode_mint(&pool.mint_a, &mint_a_data)?;
    // On pourrait stocker les décimales si on en avait besoin plus tard
    pool.mint_a_transfer_fee_bps = decoded_mint_a.transfer_fee_basis_points;

    let mint_b_data = mint_b_res?;
    let decoded_mint_b = spl_token_decoders::mint::decode_mint(&pool.mint_b, &mint_b_data)?;
    pool.mint_b_transfer_fee_bps = decoded_mint_b.transfer_fee_basis_points;
    // --- FIN DE L'AJOUT ---

    Ok(())
}