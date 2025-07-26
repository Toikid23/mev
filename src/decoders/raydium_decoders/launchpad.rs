// src/decoders/raydium_decoders/launchpad.rs

use crate::decoders::pool_operations::PoolOperations;
use bytemuck::{from_bytes, Pod, Zeroable};
use solana_sdk::pubkey::Pubkey;
use anyhow::{bail, Result, anyhow};

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
    pub total_fee_percent: f64,
    pub reserve_a: u64,
    pub reserve_b: u64,
    pub curve_type: CurveType, // Le champ clé pour la logique polymorphe
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
        total_fee_percent: 0.0,
        reserve_a: pool_struct.real_base,
        reserve_b: pool_struct.real_quote,
        curve_type: CurveType::Unknown, // Sera hydraté par le graph_engine
    })
}

// --- LOGIQUE DE POOL ---
impl PoolOperations for DecodedLaunchpadPool {
    fn get_mints(&self) -> (Pubkey, Pubkey) { (self.mint_a, self.mint_b) }
    fn get_vaults(&self) -> (Pubkey, Pubkey) { (self.vault_a, self.vault_b) }

    fn get_quote(&self, token_in_mint: &Pubkey, amount_in: u64) -> Result<u64> {
        let is_buy = *token_in_mint == self.mint_b; // True si on achète A (base) avec B (quote)

        // --- Calcul du montant de sortie en fonction du type de courbe ---
        let amount_out = match self.curve_type {
            CurveType::ConstantProduct => {
                let (in_reserve, out_reserve) = if is_buy {
                    (self.virtual_quote, self.virtual_base)
                } else {
                    (self.virtual_base, self.virtual_quote)
                };
                if in_reserve == 0 { return Ok(0); }

                let numerator = (amount_in as u128) * (out_reserve as u128);
                let denominator = (in_reserve as u128) + (amount_in as u128);
                (numerator / denominator) as u64
            },
            CurveType::FixedPrice => {
                if self.virtual_base == 0 { return Ok(0); }
                let price = self.virtual_quote as f64 / self.virtual_base as f64;
                if is_buy {
                    (amount_in as f64 / price) as u64
                } else {
                    (amount_in as f64 * price) as u64
                }
            },
            CurveType::Linear => {
                // Formule de la courbe linéaire : P = a * x + b
                // Ici, `a` est `virtual_base` et `b` est `virtual_quote`.
                // C'est une intégrale à résoudre, très complexe.
                // Pour une version exacte, on doit implémenter cette intégrale.
                // En attendant, on retourne une erreur pour ne pas donner de faux résultat.
                return Err(anyhow!("Linear curve for Launchpad is not yet implemented."));
            },
            CurveType::Unknown => {
                return Err(anyhow!("Launchpad pool curve type is unknown. Hydrate first."));
            }
        };

        // --- Application des frais (avec mathématiques sur entiers) ---
        const PRECISION: u128 = 1_000_000;
        let fee_numerator = (self.total_fee_percent * PRECISION as f64) as u128;
        let amount_out_with_fee = (amount_out as u128 * (PRECISION - fee_numerator)) / PRECISION;

        Ok(amount_out_with_fee as u64)
    }
}