// src/bin/dev_runner.rs

use solana_client::rpc_client::RpcClient;
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;
use mev::config::Config;
use mev::decoders::meteora_decoders::dlmm;

const TARGET_DLMM_POOL: &str = "5rCf1DM8LjKTw4YqhnoLcngyZYeNnQqztScTogYHAS6";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("--- Validation Chaîne Complète (IDL-based): Meteora DLMM ---");
    let config = Config::load()?;
    let rpc_client = RpcClient::new(config.solana_rpc_url);
    let pool_pubkey = Pubkey::from_str(TARGET_DLMM_POOL)?;

    // 1. Décoder le compte LbPair
    println!("\n[1/3] Décodage du LbPair...");
    let lb_pair_data = rpc_client.get_account_data(&pool_pubkey)?;
    let decoded_lb_pair = dlmm::decode_lb_pair(&pool_pubkey, &lb_pair_data)?;
    println!("-> Succès. Bin Actif ID: {}", decoded_lb_pair.active_bin_id);

    // 2. Calculer l'adresse du BinArray
    println!("\n[2/3] Calcul de l'adresse du BinArray...");
    let bin_array_address = dlmm::get_bin_array_address(&pool_pubkey, decoded_lb_pair.active_bin_id);
    println!("-> Adresse calculée: {}", bin_array_address);

    // 3. Lire et décoder le Bin actif depuis le BinArray
    println!("\n[3/3] Lecture du BinArray et extraction des réserves...");
    let bin_array_data = rpc_client.get_account_data(&bin_array_address)?;
    let decoded_bin = dlmm::decode_bin_from_bin_array(decoded_lb_pair.active_bin_id, &bin_array_data)?;

    println!("\n--- DÉCODEUR DLMM FINALISÉ ---");
    println!("   Pool: {}", decoded_lb_pair.address);
    println!("   Réserves du Bin Actif (USDC): {}", decoded_bin.amount_a);
    println!("   Réserves du Bin Actif (USDT): {}", decoded_bin.amount_b);
    println!("------------------------------");

    Ok(())
}