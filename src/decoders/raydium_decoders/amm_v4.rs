// src/decoders/raydium_decoders/amm_v4.rs

// Étape 2.1 : On importe notre nouveau "contrat"
use crate::decoders::pool_operations::PoolOperations;
use bytemuck::{from_bytes, Pod, Zeroable};
use solana_sdk::pubkey::Pubkey;
use anyhow::{bail, Result, anyhow};

// --- STRUCTURE PUBLIQUE : Elle contient maintenant les réserves ---
#[derive(Debug, Clone)]
pub struct DecodedAmmPool {
    pub address: Pubkey,
    pub mint_a: Pubkey,
    pub mint_b: Pubkey,
    pub vault_a: Pubkey,
    pub vault_b: Pubkey,
    pub total_fee_percent: f64,
    // Étape 2.2 : On ajoute les champs pour les réserves.
    // Ils seront mis à jour par le graph_engine avant tout calcul.
    pub reserve_a: u64,
    pub reserve_b: u64,
}

// --- STRUCTURES BRUTES (ne changent pas, assurez-vous qu'elles sont complètes) ---
#[repr(C, packed)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
struct Fees {
    pub min_separate_numerator: u64, pub min_separate_denominator: u64,
    pub trade_fee_numerator: u64, pub trade_fee_denominator: u64,
    pub pnl_numerator: u64, pub pnl_denominator: u64,
    pub swap_fee_numerator: u64, pub swap_fee_denominator: u64,
}

#[repr(C, packed)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
struct OutPutData {
    pub need_take_pnl_coin: u64, pub need_take_pnl_pc: u64,
    pub total_pnl_pc: u64, pub total_pnl_coin: u64,
    pub pool_open_time: u64, pub punish_pc_amount: u64,
    pub punish_coin_amount: u64, pub orderbook_to_init_time: u64,
    pub swap_coin_in_amount: u128, pub swap_pc_out_amount: u128,
    pub swap_take_pc_fee: u64, pub swap_pc_in_amount: u128,
    pub swap_coin_out_amount: u128, pub swap_take_coin_fee: u64,
}

#[repr(C, packed)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
struct AmmInfoData {
    pub status: u64, pub nonce: u64, pub order_num: u64, pub depth: u64,
    pub coin_decimals: u64, pub pc_decimals: u64, pub state: u64,
    pub reset_flag: u64, pub min_size: u64, pub vol_max_cut_ratio: u64,
    pub amount_wave: u64, pub coin_lot_size: u64, pub pc_lot_size: u64,
    pub min_price_multiplier: u64, pub max_price_multiplier: u64,
    pub sys_decimal_value: u64, pub fees: Fees, pub out_put: OutPutData,
    pub token_coin: Pubkey, pub token_pc: Pubkey, pub coin_mint: Pubkey,
    pub pc_mint: Pubkey, pub lp_mint: Pubkey, pub open_orders: Pubkey,
    pub market: Pubkey, pub serum_dex: Pubkey, pub target_orders: Pubkey,
    pub withdraw_queue: Pubkey, pub token_temp_lp: Pubkey,
    pub amm_owner: Pubkey, pub lp_amount: u64, pub client_order_id: u64,
    pub padding: [u64; 2],
}

/// Décode un compte Raydium AMM V4.
pub fn decode_pool(address: &Pubkey, data: &[u8]) -> Result<DecodedAmmPool> {
    if data.len() != std::mem::size_of::<AmmInfoData>() {
        bail!(
            "AMM V4 data length mismatch. Expected {}, got {}.",
            std::mem::size_of::<AmmInfoData>(),
            data.len()
        );
    }
    let pool_struct: &AmmInfoData = from_bytes(data);
    let total_fee_percent = pool_struct.fees.trade_fee_numerator as f64 / pool_struct.fees.trade_fee_denominator as f64;

    Ok(DecodedAmmPool {
        address: *address,
        mint_a: pool_struct.coin_mint,
        mint_b: pool_struct.pc_mint,
        vault_a: pool_struct.token_coin,
        vault_b: pool_struct.token_pc,
        total_fee_percent,
        reserve_a: 0, // Les réserves sont initialisées à 0
        reserve_b: 0,
    })
}

// --- Étape 2.3 : On implémente le "contrat" pour notre struct ---
impl PoolOperations for DecodedAmmPool {
    fn get_mints(&self) -> (Pubkey, Pubkey) {
        (self.mint_a, self.mint_b)
    }

    fn get_vaults(&self) -> (Pubkey, Pubkey) {
        (self.vault_a, self.vault_b)
    }

    fn get_quote(&self, token_in_mint: &Pubkey, amount_in: u64) -> Result<u64> {
        if self.reserve_a == 0 || self.reserve_b == 0 {
            return Ok(0); // Pas de liquidité, on retourne 0
        }

        let (in_reserve, out_reserve) = if *token_in_mint == self.mint_a {
            (self.reserve_a, self.reserve_b)
        } else if *token_in_mint == self.mint_b {
            (self.reserve_b, self.reserve_a)
        } else {
            return Err(anyhow!("Input token does not belong to this pool."));
        };

        // --- MATHÉMATIQUES OPTIMISÉES AVEC u128 ---
        const PRECISION: u128 = 1_000_000; // On choisit une précision d'un million

        // On convertit le pourcentage de frais en un numérateur entier
        // Ex: 0.0025 (0.25%) devient 2500
        let fee_numerator = (self.total_fee_percent * PRECISION as f64) as u128;

        // Calcul du montant après frais, en utilisant uniquement des entiers
        let amount_in_with_fee = amount_in as u128 * (PRECISION - fee_numerator) / PRECISION;

        // Calcul du montant de sortie avec la formule x*y=k
        let numerator = amount_in_with_fee * out_reserve as u128;
        let denominator = in_reserve as u128 + amount_in_with_fee;

        if denominator == 0 {
            return Ok(0); // Evite la division par zéro
        }

        Ok((numerator / denominator) as u64)
    }
}