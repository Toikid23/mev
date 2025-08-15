// DANS : src/bin/setup_wallet.rs (VERSION CORRIGÉE POUR TOKEN-2022)

use anyhow::Result;
use mev::config::Config;
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::{
    pubkey::Pubkey,
    signer::{keypair::Keypair, Signer},
    transaction::Transaction,
};
use std::str::FromStr;

// --- LE SEUL ENDROIT À MODIFIER ---
// Collez ici l'adresse du MINT du jeton pour lequel vous voulez créer un ATA.
// C'est déjà la bonne adresse, celle du pool CLMM.
const MINT_ADDRESS: &str = "Ey59PH7Z4BFU4HjyKnyMdWt5GGN76KazTAwQihoUXRnk";

#[tokio::main]
async fn main() -> Result<()> {
    println!("--- Outil de Création d'ATA ---");

    // 1. Charger la configuration et le portefeuille payeur
    let config = Config::load()?;
    let rpc_client = RpcClient::new(config.solana_rpc_url);
    let payer = Keypair::from_base58_string(&config.payer_private_key);
    let mint_to_create_ata_for = Pubkey::from_str(MINT_ADDRESS)?;

    println!("Portefeuille Payeur: {}", payer.pubkey());
    println!("Mint du Jeton Cible: {}", mint_to_create_ata_for);

    // 2. Construire l'instruction de création d'ATA
    // --- LA CORRECTION EST ICI ---
    // On utilise l'ID du programme Token-2022 car le mint lui appartient.
    let create_ata_instruction = spl_associated_token_account::instruction::create_associated_token_account(
        &payer.pubkey(),
        &payer.pubkey(),
        &mint_to_create_ata_for,
        &spl_token_2022::id(), // <-- ON UTILISE LE BON PROGRAMME !
    );

    // 3. Créer, signer et envoyer la transaction
    let recent_blockhash = rpc_client.get_latest_blockhash().await?;
    let mut transaction = Transaction::new_with_payer(
        &[create_ata_instruction],
        Some(&payer.pubkey()),
    );
    transaction.sign(&[&payer], recent_blockhash);

    println!("\nEnvoi de la transaction pour créer l'ATA...");

    let signature = rpc_client.send_and_confirm_transaction(&transaction).await?;

    // 4. Afficher le résultat
    let ata_address = spl_associated_token_account::get_associated_token_address(&payer.pubkey(), &mint_to_create_ata_for);

    println!("\n✅ SUCCÈS !");
    println!("L'ATA a été créé avec succès.");
    println!("Adresse de l'ATA: {}", ata_address);
    println!("Signature de la transaction: {}", signature);
    println!("Vous pouvez vérifier sur Solscan: https://solscan.io/tx/{}", signature);

    Ok(())
}