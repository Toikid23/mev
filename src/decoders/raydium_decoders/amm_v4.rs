// src/decoders/raydium_decoders/amm_v4.rs

// Étape 2.1 : On importe notre nouveau "contrat"
use crate::decoders::pool_operations::PoolOperations;
use bytemuck::{from_bytes, Pod, Zeroable};
use solana_sdk::pubkey::Pubkey;
use anyhow::{bail, Result, anyhow};
use solana_client::nonblocking::rpc_client::RpcClient;
use crate::decoders::spl_token_decoders;
use solana_sdk::{
    instruction::{AccountMeta, Instruction},
    sysvar,
};
use solana_sdk::pubkey;


// --- STRUCTURE PUBLIQUE : Elle contient maintenant les réserves ---
#[derive(Debug, Clone)]
pub struct DecodedAmmPool {
    // --- Champs de base ---
    pub address: Pubkey,
    pub nonce: u64,

    // --- Mints & Vaults du Pool ---
    pub mint_a: Pubkey,
    pub mint_b: Pubkey,
    pub vault_a: Pubkey,
    pub vault_b: Pubkey,

    // --- Infos du Marché Associé ---
    pub market: Pubkey,
    pub market_program_id: Pubkey,
    pub market_bids: Pubkey,
    pub market_asks: Pubkey,
    pub market_event_queue: Pubkey,
    pub market_coin_vault: Pubkey,
    pub market_pc_vault: Pubkey,
    pub market_vault_signer: Pubkey,

    // --- Autres comptes requis ---
    pub open_orders: Pubkey,
    pub target_orders: Pubkey, // <-- AJOUTÉ

    // --- Données pour le calcul de quote ---
    pub mint_a_decimals: u8,
    pub mint_b_decimals: u8,
    pub mint_a_transfer_fee_bps: u16,
    pub mint_b_transfer_fee_bps: u16,
    pub trade_fee_numerator: u64,
    pub trade_fee_denominator: u64,
    pub reserve_a: u64,
    pub reserve_b: u64,
}

// Dans amm_v4.rs, après la struct DecodedAmmPool
impl DecodedAmmPool {
    /// Calcule et retourne les frais de pool sous forme de pourcentage lisible.
    pub fn fee_as_percent(&self) -> f64 {
        if self.trade_fee_denominator == 0 { return 0.0; }
        (self.trade_fee_numerator as f64 / self.trade_fee_denominator as f64) * 100.0
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
    // --- CORRECTION ---
    // On vérifie que les données sont AU MOINS de la taille attendue, pas exactement égales.
    if data.len() < std::mem::size_of::<AmmInfoData>() {
        bail!(
            "AMM V4 data length mismatch. Expected at least {}, got {}.", // Message d'erreur mis à jour
            std::mem::size_of::<AmmInfoData>(),
            data.len()
        );
    }
    // --- FIN DE LA CORRECTION ---

    // Le reste du décodage ne prend que la partie des données qui nous intéresse.
    let pool_struct: &AmmInfoData = from_bytes(&data[..std::mem::size_of::<AmmInfoData>()]);

    if pool_struct.status == 0 {
        bail!("Pool {} is not initialized (status is 0).", address);
    }

    // Le reste de la fonction est inchangé
    Ok(DecodedAmmPool {
        address: *address,
        nonce: pool_struct.nonce,
        market: pool_struct.market,
        market_program_id: pool_struct.serum_dex,
        open_orders: pool_struct.open_orders,
        target_orders: pool_struct.target_orders,
        mint_a: pool_struct.coin_mint,
        mint_b: pool_struct.pc_mint,
        vault_a: pool_struct.token_coin,
        vault_b: pool_struct.token_pc,
        trade_fee_numerator: pool_struct.fees.trade_fee_numerator,
        trade_fee_denominator: pool_struct.fees.trade_fee_denominator,

        // Initialisation des champs hydratés
        mint_a_decimals: 0,
        mint_b_decimals: 0,
        mint_a_transfer_fee_bps: 0,
        mint_b_transfer_fee_bps: 0,
        reserve_a: 0,
        reserve_b: 0,
        market_bids: Pubkey::default(),
        market_asks: Pubkey::default(),
        market_event_queue: Pubkey::default(),
        market_coin_vault: Pubkey::default(),
        market_pc_vault: Pubkey::default(),
        market_vault_signer: Pubkey::default(),
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

    fn get_quote(&self, token_in_mint: &Pubkey, amount_in: u64, _current_timestamp: i64) -> Result<u64> {
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
        let amount_in_with_fees = (amount_in_after_transfer_fee as u128)
            .saturating_mul(self.trade_fee_denominator.saturating_sub(self.trade_fee_numerator) as u128)
            / (self.trade_fee_denominator as u128);

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
    // La première partie (fetch des comptes et parsing des vaults/mints du pool) est correcte.
    let accounts_to_fetch = [pool.vault_a, pool.vault_b, pool.mint_a, pool.mint_b, pool.market];
    let accounts_data = rpc_client.get_multiple_accounts(&accounts_to_fetch).await?;

    let vault_a_account = accounts_data[0].as_ref().ok_or_else(|| anyhow!("AMM V4 Vault A not found"))?;
    pool.reserve_a = u64::from_le_bytes(vault_a_account.data[64..72].try_into()?);
    let vault_b_account = accounts_data[1].as_ref().ok_or_else(|| anyhow!("AMM V4 Vault B not found"))?;
    pool.reserve_b = u64::from_le_bytes(vault_b_account.data[64..72].try_into()?);
    let mint_a_data = accounts_data[2].as_ref().ok_or_else(|| anyhow!("Mint A not found"))?;
    let decoded_mint_a = spl_token_decoders::mint::decode_mint(&pool.mint_a, &mint_a_data.data)?;
    pool.mint_a_decimals = decoded_mint_a.decimals;
    pool.mint_a_transfer_fee_bps = decoded_mint_a.transfer_fee_basis_points;
    let mint_b_data = accounts_data[3].as_ref().ok_or_else(|| anyhow!("Mint B not found"))?;
    let decoded_mint_b = spl_token_decoders::mint::decode_mint(&pool.mint_b, &mint_b_data.data)?;
    pool.mint_b_decimals = decoded_mint_b.decimals;
    pool.mint_b_transfer_fee_bps = decoded_mint_b.transfer_fee_basis_points;

    // --- PARSING MANUEL AVEC LES OFFSETS DE CERTITUDE DU SDK ---
    let market_account = accounts_data[4].as_ref().ok_or_else(|| anyhow!("Market account not found"))?;
    let market_data = &market_account.data;

    // Basé sur MARKET_STATE_LAYOUT_V3 de votre SDK
    const VAULT_SIGNER_NONCE_OFFSET: usize = 5 + 8 + 32; // 45
    const BASE_VAULT_OFFSET: usize = VAULT_SIGNER_NONCE_OFFSET + 8 + 32 + 32; // 117
    const QUOTE_VAULT_OFFSET: usize = BASE_VAULT_OFFSET + 32 + 8 + 8; // 165
    const EVENT_QUEUE_OFFSET: usize = QUOTE_VAULT_OFFSET + 32 + 8 + 8 + 8 + 32; // 253
    const BIDS_OFFSET: usize = EVENT_QUEUE_OFFSET + 32; // 285
    const ASKS_OFFSET: usize = BIDS_OFFSET + 32; // 317

    pool.market_coin_vault = Pubkey::new_from_array(market_data[BASE_VAULT_OFFSET..BASE_VAULT_OFFSET+32].try_into()?);
    pool.market_pc_vault = Pubkey::new_from_array(market_data[QUOTE_VAULT_OFFSET..QUOTE_VAULT_OFFSET+32].try_into()?);
    pool.market_event_queue = Pubkey::new_from_array(market_data[EVENT_QUEUE_OFFSET..BIDS_OFFSET].try_into()?);
    pool.market_bids = Pubkey::new_from_array(market_data[BIDS_OFFSET..ASKS_OFFSET].try_into()?);
    pool.market_asks = Pubkey::new_from_array(market_data[ASKS_OFFSET..ASKS_OFFSET + 32].try_into()?);

    let vault_signer_nonce = u64::from_le_bytes(market_data[VAULT_SIGNER_NONCE_OFFSET..VAULT_SIGNER_NONCE_OFFSET + 8].try_into()?);

    // La dérivation du vault signer est un cas particulier documenté par Serum/OpenBook
    let market_address_bytes = pool.market.to_bytes();
    let seeds: &[&[u8]] = &[&market_address_bytes, &vault_signer_nonce.to_le_bytes()];

    pool.market_vault_signer = Pubkey::create_program_address(
        seeds,
        &pool.market_program_id,
    )?;

    Ok(())
}

pub const RAYDIUM_AMM_V4_PROGRAM_ID: Pubkey = pubkey!("675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8");

pub fn create_swap_instruction(
    pool: &DecodedAmmPool,
    user_source_token_account: &Pubkey,
    user_destination_token_account: &Pubkey,
    user_owner: &Pubkey,
    amount_in: u64,
    minimum_amount_out: u64,
) -> Result<Instruction> {
    let mut instruction_data = vec![9];
    instruction_data.extend_from_slice(&amount_in.to_le_bytes());
    instruction_data.extend_from_slice(&minimum_amount_out.to_le_bytes());

    let (amm_authority, _) = Pubkey::find_program_address(
        &[b"amm authority"],
        &RAYDIUM_AMM_V4_PROGRAM_ID,
    );

    // CETTE LISTE EST UNE TRADUCTION DIRECTE DE instruction.ts DU SDK
    let keys = vec![
        // 0. `[]` spl-token program
        AccountMeta::new_readonly(spl_token::id(), false),
        // 1. `[writable]` AMM account
        AccountMeta::new(pool.address, false),
        // 2. `[]` AMM authority
        AccountMeta::new_readonly(amm_authority, false),
        // 3. `[writable]` AMM open orders
        AccountMeta::new(pool.open_orders, false),
        // 4. `[writable]` AMM target orders
        AccountMeta::new(pool.target_orders, false),
        // 5. `[writable]` AMM coin vault
        AccountMeta::new(pool.vault_a, false),
        // 6. `[writable]` AMM pc vault
        AccountMeta::new(pool.vault_b, false),
        // 7. `[]` Market program id
        AccountMeta::new_readonly(pool.market_program_id, false),
        // 8. `[writable]` Market account
        AccountMeta::new(pool.market, false),
        // 9. `[writable]` Market bids
        AccountMeta::new(pool.market_bids, false),
        // 10. `[writable]` Market asks
        AccountMeta::new(pool.market_asks, false),
        // 11. `[writable]` Market event queue
        AccountMeta::new(pool.market_event_queue, false),
        // 12. `[writable]` Market coin vault
        AccountMeta::new(pool.market_coin_vault, false),
        // 13. `[writable]` Market pc vault
        AccountMeta::new(pool.market_pc_vault, false),
        // 14. `[]` Market vault signer
        AccountMeta::new_readonly(pool.market_vault_signer, false),
        // 15. `[writable]` User source token account
        AccountMeta::new(*user_source_token_account, false),
        // 16. `[writable]` User destination token account
        AccountMeta::new(*user_destination_token_account, false),
        // 17. `[signer]` User owner
        AccountMeta::new_readonly(*user_owner, true),
    ];

    Ok(Instruction {
        program_id: RAYDIUM_AMM_V4_PROGRAM_ID,
        accounts: keys,
        data: instruction_data,
    })
}