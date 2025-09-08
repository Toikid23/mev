use crate::{
    data_pipeline::onchain_scanner,
    decoders::{raydium, orca, meteora, pump},
    filtering::{PoolIdentity, cache::PoolCache},
    rpc::ResilientRpcClient, // <-- NOUVEL IMPORT
};
use anyhow::{Context, Result};
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;

// La fonction devient `pub async fn` et prend notre client
pub async fn run_census(rpc_client: &ResilientRpcClient) -> Result<()> {
    println!("\n--- [Recensement] Démarrage du scan complet de la blockchain pour les pools ---");

    let mut all_identities = Vec::new();

    let raydium_amm_v4_id = Pubkey::from_str("675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8")?;
    let raydium_cpmm_id = Pubkey::from_str("CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C")?;
    let raydium_clmm_id = Pubkey::from_str("CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK")?;

    // Les appels doivent maintenant être `await`
    scan_and_decode_pools(&mut all_identities, rpc_client, raydium_amm_v4_id, PoolType::RaydiumAmmV4).await?;
    scan_and_decode_pools(&mut all_identities, rpc_client, raydium_cpmm_id, PoolType::RaydiumCpmm).await?;
    scan_and_decode_pools(&mut all_identities, rpc_client, raydium_clmm_id, PoolType::RaydiumClmm).await?;

    let orca_whirlpool_id = Pubkey::from_str("whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc")?;
    scan_and_decode_pools(&mut all_identities, rpc_client, orca_whirlpool_id, PoolType::OrcaWhirlpool).await?;

    let meteora_damm_v1_id = Pubkey::from_str("Eo7WjKq67rjJQSZxS6z3YkapzY3eMj6Xy8X5EQVn5UaB")?;
    let meteora_damm_v2_id = Pubkey::from_str("cpamdpZCGKUy5JxQXB4dcpGPiikHawvSWAd6mEn1sGG")?;
    let meteora_dlmm_id = Pubkey::from_str("LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo")?;

    scan_and_decode_pools(&mut all_identities, rpc_client, meteora_damm_v1_id, PoolType::MeteoraDammV1).await?;
    scan_and_decode_pools(&mut all_identities, rpc_client, meteora_damm_v2_id, PoolType::MeteoraDammV2).await?;
    scan_and_decode_pools(&mut all_identities, rpc_client, meteora_dlmm_id, PoolType::MeteoraDlmm).await?;

    let pump_amm_id = Pubkey::from_str("pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA")?;
    scan_and_decode_pools(&mut all_identities, rpc_client, pump_amm_id, PoolType::PumpAmm).await?;

    println!("\n[Recensement] Scan terminé. Total de {} pools trouvés sur tous les protocoles.", all_identities.len());

    PoolCache::save(&all_identities)
        .context("Échec de la sauvegarde du cache de l'univers des pools")?;

    Ok(())
}


/// Un enum interne pour gérer la logique de décodage polymorphe.
enum PoolType {
    RaydiumAmmV4, // <-- AJOUT
    RaydiumCpmm,
    RaydiumClmm,
    OrcaWhirlpool,
    MeteoraDammV1, // <-- AJOUT
    MeteoraDammV2, // <-- AJOUT
    MeteoraDlmm,
    PumpAmm,
}

/// Scanne un programme DEX spécifique et décode les pools trouvés pour en extraire leur `PoolIdentity`.
// Cette fonction devient aussi `async`
async fn scan_and_decode_pools(
    identities: &mut Vec<PoolIdentity>,
    rpc_client: &ResilientRpcClient,
    program_id: Pubkey,
    pool_type: PoolType,
) -> Result<()> {
    let raw_pools_data = onchain_scanner::find_pools_by_program_id_with_filters(
        rpc_client,
        &program_id.to_string(),
        None,
    ).await?; // <-- Appel `await` ici

    let mut successful_decodes = 0;
    for raw_pool in &raw_pools_data {
        if let Ok(identity) = decode_identity(&raw_pool.address, &raw_pool.data, &program_id, &pool_type) {
            identities.push(identity);
            successful_decodes += 1;
        }
    }

    println!("[Recensement] -> Programme {}: {} comptes trouvés, {} pools valides décodés.", program_id, raw_pools_data.len(), successful_decodes);
    Ok(())
}

/// La fonction de décodage "légère" qui ne crée que la `PoolIdentity`.
fn decode_identity(
    address: &Pubkey,
    data: &[u8],
    program_id: &Pubkey,
    pool_type: &PoolType,
) -> Result<PoolIdentity> {
    match pool_type {
        // --- AJOUTS ---
        PoolType::RaydiumAmmV4 => {
            let pool = raydium::amm_v4::decode_pool(address, data)?;
            Ok(PoolIdentity {
                address: *address,
                mint_a: pool.mint_a,
                mint_b: pool.mint_b,
                accounts_to_watch: vec![pool.vault_a, pool.vault_b], // AMM -> surveiller les vaults
            })
        }
        PoolType::MeteoraDammV1 => {
            let pool = meteora::damm_v1::decode_pool(address, data)?;
            Ok(PoolIdentity {
                address: *address,
                mint_a: pool.mint_a,
                mint_b: pool.mint_b,
                // Les vaults principaux sont les comptes à surveiller pour les swaps
                accounts_to_watch: vec![pool.vault_a, pool.vault_b],
            })
        }
        PoolType::MeteoraDammV2 => {
            let pool = meteora::damm_v2::decode_pool(address, data)?;
            Ok(PoolIdentity {
                address: *address,
                mint_a: pool.mint_a,
                mint_b: pool.mint_b,
                accounts_to_watch: vec![*address], // CLMM-like -> surveiller le pool state
            })
        }
        // --- FIN DES AJOUTS ---

        PoolType::RaydiumCpmm => {
            let pool = raydium::cpmm::decode_pool(address, data)?;
            Ok(PoolIdentity {
                address: *address,
                mint_a: pool.token_0_mint,
                mint_b: pool.token_1_mint,
                accounts_to_watch: vec![pool.token_0_vault, pool.token_1_vault],
            })
        }
        PoolType::RaydiumClmm => {
            let pool = raydium::clmm::decode_pool(address, data, program_id)?;
            Ok(PoolIdentity {
                address: *address,
                mint_a: pool.mint_a,
                mint_b: pool.mint_b,
                accounts_to_watch: vec![*address],
            })
        }
        PoolType::OrcaWhirlpool => {
            let pool = orca::whirlpool::decode_pool(address, data)?;
            Ok(PoolIdentity {
                address: *address,
                mint_a: pool.mint_a,
                mint_b: pool.mint_b,
                accounts_to_watch: vec![*address],
            })
        }
        PoolType::MeteoraDlmm => {
            let pool = meteora::dlmm::decode_lb_pair(address, data, program_id)?;
            Ok(PoolIdentity {
                address: *address,
                mint_a: pool.mint_a,
                mint_b: pool.mint_b,
                accounts_to_watch: vec![*address],
            })
        }
        PoolType::PumpAmm => {
            let pool = pump::amm::decode_pool(address, data)?;
            Ok(PoolIdentity {
                address: *address,
                mint_a: pool.mint_a,
                mint_b: pool.mint_b,
                accounts_to_watch: vec![pool.vault_a, pool.vault_b],
            })
        }
    }
}