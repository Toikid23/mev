// src/decoders/raydium_decoders/amm_v4.rs

// Étape 2.1 : On importe notre nouveau "contrat"
use crate::decoders::pool_operations::PoolOperations;
use bytemuck::{from_bytes, Pod, Zeroable};
use solana_sdk::pubkey::Pubkey;
use anyhow::{bail, Result, anyhow};
use solana_client::nonblocking::rpc_client::RpcClient;
use crate::decoders::spl_token_decoders;

// --- STRUCTURE PUBLIQUE : Elle contient maintenant les réserves ---
#[derive(Debug, Clone)]
pub struct DecodedAmmPool {
    pub address: Pubkey,
    pub mint_a: Pubkey,
    pub mint_b: Pubkey,
    pub mint_a_transfer_fee_bps: u16,
    pub mint_b_transfer_fee_bps: u16,
    pub vault_a: Pubkey,
    pub vault_b: Pubkey,
    pub total_fee_percent: f64,
    // Étape 2.2 : On ajoute les champs pour les réserves.
    // Ils seront mis à jour par le graph_engine avant tout calcul.
    pub reserve_a: u64,
    pub reserve_b: u64,
}

// Dans amm_v4.rs, après la struct DecodedAmmPool
impl DecodedAmmPool {
    /// Calcule et retourne les frais de pool sous forme de pourcentage lisible.
    pub fn fee_as_percent(&self) -> f64 {
        // total_fee_percent est déjà un ratio (ex: 0.0025 pour 0.25%)
        self.total_fee_percent * 100.0
    }
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
        mint_a_transfer_fee_bps: 0,
        mint_b_transfer_fee_bps: 0,
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
        // --- 1. Appliquer les frais de transfert sur l'INPUT ---
        let (in_mint_fee_bps, out_mint_fee_bps, in_reserve, out_reserve) = if *token_in_mint == self.mint_a {
            (self.mint_a_transfer_fee_bps, self.mint_b_transfer_fee_bps, self.reserve_a, self.reserve_b)
        } else if *token_in_mint == self.mint_b {
            (self.mint_b_transfer_fee_bps, self.mint_a_transfer_fee_bps, self.reserve_b, self.reserve_a)
        } else {
            return Err(anyhow!("Input token does not belong to this pool."));
        };

        let fee_on_input = (amount_in as u128 * in_mint_fee_bps as u128) / 10000;
        let amount_in_after_transfer_fee = amount_in.saturating_sub(fee_on_input as u64);

        if in_reserve == 0 || out_reserve == 0 {
            return Ok(0);
        }

        // --- 2. Calculer le swap avec le montant NET ---
        const PRECISION: u128 = 1_000_000;
        let pool_fee_numerator = (self.total_fee_percent * PRECISION as f64) as u128;
        let amount_in_with_fees = (amount_in_after_transfer_fee as u128 * (PRECISION - pool_fee_numerator)) / PRECISION;

        let numerator = amount_in_with_fees * out_reserve as u128;
        let denominator = in_reserve as u128 + amount_in_with_fees;
        if denominator == 0 { return Ok(0); }
        let gross_amount_out = (numerator / denominator) as u64;

        // --- 3. Appliquer les frais de transfert sur l'OUTPUT ---
        let fee_on_output = (gross_amount_out as u128 * out_mint_fee_bps as u128) / 10000;
        let final_amount_out = gross_amount_out.saturating_sub(fee_on_output as u64);

        Ok(final_amount_out)
    }
}

pub async fn hydrate(pool: &mut DecodedAmmPool, rpc_client: &RpcClient) -> Result<()> {
    // --- LA CORRECTION EST ICI ---
    // On crée une variable qui va "posséder" le slice de pubkeys.
    // Sa durée de vie est maintenant celle de toute la fonction hydrate.
    let vaults_to_fetch = [pool.vault_a, pool.vault_b];

    // On fetch tout en parallèle : les vaults et les mints
    let (vault_accounts_res, mint_a_res, mint_b_res) = tokio::join!(
         // On passe une référence à notre variable qui a une longue durée de vie
         rpc_client.get_multiple_accounts(&vaults_to_fetch),
         rpc_client.get_account_data(&pool.mint_a),
         rpc_client.get_account_data(&pool.mint_b)
    );

    // Traitement des vaults
    let vault_accounts = vault_accounts_res?;
    let vault_a_account = vault_accounts[0]
        .as_ref()
        .ok_or_else(|| anyhow!("AMM V4 Vault A not found for pool {}", pool.address))?;
    pool.reserve_a = u64::from_le_bytes(vault_a_account.data[64..72].try_into()?);

    let vault_b_account = vault_accounts[1]
        .as_ref()
        .ok_or_else(|| anyhow!("AMM V4 Vault B not found for pool {}", pool.address))?;
    pool.reserve_b = u64::from_le_bytes(vault_b_account.data[64..72].try_into()?);

    // --- AJOUT DE LA LOGIQUE D'HYDRATATION DES MINTS ---
    let mint_a_data = mint_a_res?;
    let decoded_mint_a = spl_token_decoders::mint::decode_mint(&pool.mint_a, &mint_a_data)?;
    pool.mint_a_transfer_fee_bps = decoded_mint_a.transfer_fee_basis_points;

    let mint_b_data = mint_b_res?;
    let decoded_mint_b = spl_token_decoders::mint::decode_mint(&pool.mint_b, &mint_b_data)?;
    pool.mint_b_transfer_fee_bps = decoded_mint_b.transfer_fee_basis_points;
    // --- FIN DE L'AJOUT ---

    Ok(())
}