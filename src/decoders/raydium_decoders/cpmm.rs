// src/decoders/raydium_decoders/cpmm.rs

use crate::decoders::pool_operations::PoolOperations; // On importe le contrat
use bytemuck::{from_bytes, Pod, Zeroable};
use solana_sdk::pubkey::Pubkey;
use anyhow::{bail, Result, anyhow};
use solana_client::nonblocking::rpc_client::RpcClient;
use crate::decoders::raydium_decoders::amm_config;
use crate::decoders::spl_token_decoders;

// Discriminator pour les comptes PoolState du programme CPMM
const CPMM_POOL_STATE_DISCRIMINATOR: [u8; 8] = [247, 237, 227, 245, 215, 195, 222, 70];

// --- STRUCTURE DE SORTIE PROPRE ---
// Contient les infos décodées et utiles du PoolState CPMM.
// Notez que nous extrayons l'adresse de l'AmmConfig pour une lecture ultérieure.
#[derive(Debug, Clone)]
pub struct DecodedCpmmPool {
    pub address: Pubkey,
    pub amm_config: Pubkey, // Pour aller chercher les frais plus tard
    pub token_0_mint: Pubkey,
    pub token_1_mint: Pubkey,
    pub token_0_vault: Pubkey,
    pub token_1_vault: Pubkey,
    pub mint_a_transfer_fee_bps: u16,
    pub mint_b_transfer_fee_bps: u16,
    pub status: u8,
    // Les champs "intelligents"
    pub trade_fee_rate: u64,
    pub reserve_a: u64,
    pub reserve_b: u64,
}

// Dans cpmm.rs, après la struct DecodedCpmmPool
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
    pub padding: [u64; 31],
}

/// Tente de décoder les données brutes d'un compte Raydium CPMM PoolState.
pub fn decode_pool(address: &Pubkey, data: &[u8]) -> Result<DecodedCpmmPool> {
    // Étape 1: Vérifier le discriminator
    if data.get(..8) != Some(&CPMM_POOL_STATE_DISCRIMINATOR) {
        bail!("Invalid discriminator. Not a Raydium CPMM PoolState account.");
    }

    let data_slice = &data[8..];

    // Étape 2: Vérifier la taille
    if data_slice.len() != std::mem::size_of::<CpmmPoolStateData>() {
        bail!(
            "CPMM PoolState data length mismatch. Expected {}, got {}.",
            std::mem::size_of::<CpmmPoolStateData>(),
            data_slice.len()
        );
    }

    // Étape 3: "Caster" les données
    let pool_struct: &CpmmPoolStateData = from_bytes(data_slice);



    // Étape 4: Créer la sortie propre et unifiée
    Ok(DecodedCpmmPool {
        address: *address,
        amm_config: pool_struct.amm_config,
        token_0_mint: pool_struct.token_0_mint,
        token_1_mint: pool_struct.token_1_mint,
        token_0_vault: pool_struct.token_0_vault,
        token_1_vault: pool_struct.token_1_vault,
        status: pool_struct.status,
        mint_a_transfer_fee_bps: 0,
        mint_b_transfer_fee_bps: 0,
        // Les champs "intelligents" sont initialisés à 0
        trade_fee_rate: 0,
        reserve_a: 0,
        reserve_b: 0,
    })
}

// --- IMPLEMENTATION DE LA LOGIQUE DU POOL ---
impl PoolOperations for DecodedCpmmPool {
    fn get_mints(&self) -> (Pubkey, Pubkey) {
        (self.token_0_mint, self.token_1_mint)
    }

    fn get_vaults(&self) -> (Pubkey, Pubkey) {
        (self.token_0_vault, self.token_1_vault)
    }

    fn get_quote(&self, token_in_mint: &Pubkey, amount_in: u64) -> Result<u64> {
        // --- 1. Appliquer les frais de transfert sur l'INPUT ---
        let (in_mint_fee_bps, out_mint_fee_bps, in_reserve, out_reserve) = if *token_in_mint == self.token_0_mint {
            (self.mint_a_transfer_fee_bps, self.mint_b_transfer_fee_bps, self.reserve_a, self.reserve_b)
        } else if *token_in_mint == self.token_1_mint {
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
        const FEE_PRECISION: u128 = 1_000_000;
        let amount_in_with_fees = (amount_in_after_transfer_fee as u128)
            .saturating_mul(FEE_PRECISION.saturating_sub(self.trade_fee_rate as u128))
            / FEE_PRECISION;

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

pub async fn hydrate(pool: &mut DecodedCpmmPool, rpc_client: &RpcClient) -> Result<()> {
    // Lance les 3 appels réseau indépendants (config, vault A, vault B) en parallèle
    let (config_res, vault_a_res, vault_b_res, mint_a_res, mint_b_res) = tokio::join!(
        rpc_client.get_account_data(&pool.amm_config),
        rpc_client.get_account_data(&pool.token_0_vault),
        rpc_client.get_account_data(&pool.token_1_vault),
        rpc_client.get_account_data(&pool.token_0_mint), // <-- NOUVEAU
        rpc_client.get_account_data(&pool.token_1_mint)
    );

    // Traite le résultat de la config pour obtenir les frais de pool
    let config_data = config_res?;
    let decoded_config = amm_config::decode_config(&config_data)?;
    pool.trade_fee_rate = decoded_config.trade_fee_rate;

    // Traite les résultats des vaults pour obtenir les réserves
    let vault_a_data = vault_a_res?;
    pool.reserve_a = u64::from_le_bytes(vault_a_data[64..72].try_into()?);
    let vault_b_data = vault_b_res?;
    pool.reserve_b = u64::from_le_bytes(vault_b_data[64..72].try_into()?);

    // --- AJOUT DE LA LOGIQUE D'HYDRATATION DES MINTS ---
    let mint_a_data = mint_a_res?;
    let decoded_mint_a = spl_token_decoders::mint::decode_mint(&pool.token_0_mint, &mint_a_data)?;
    // On pourrait stocker les décimales dans le pool si nécessaire, mais pour l'instant on ne s'en sert pas
    pool.mint_a_transfer_fee_bps = decoded_mint_a.transfer_fee_basis_points;

    let mint_b_data = mint_b_res?;
    let decoded_mint_b = spl_token_decoders::mint::decode_mint(&pool.token_1_mint, &mint_b_data)?;
    pool.mint_b_transfer_fee_bps = decoded_mint_b.transfer_fee_basis_points;
    // --- FIN DE L'AJOUT ---

    Ok(())
}