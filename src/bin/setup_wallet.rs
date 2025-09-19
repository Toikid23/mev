#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

use anyhow::{bail, Result};
use mev::config::Config;
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::{
    pubkey::Pubkey,
    signer::{keypair::Keypair, Signer},
    transaction::Transaction,
};
use std::str::FromStr;
use anyhow::anyhow;
use tracing::info;
use mev::monitoring::logging;

// --- LE SEUL ENDROIT À MODIFIER ---
// Collez ici l'adresse du MINT du jeton pour lequel vous voulez créer un ATA.


const MINT_ADDRESS: &str = "CXbypcjnbV7A5b6XqfkUCUkLiD3czHXoxAsWBYa4pump";

// Pour le test CLMM, utilisez celle-ci :
// const MINT_ADDRESS: &str = "Ey59PH7Z4BFU4HjyKnyMdWt5GGN76KazTAwQihoUXRnk"; // LAUNCHCOIN (Token-2022)

#[tokio::main]
async fn main() -> Result<()> {
    logging::setup_logging();
    info!("--- Outil de Création d'ATA (Universel) ---");

    // 1. Charger la configuration
    let config = Config::load()?;
    let rpc_client = RpcClient::new(config.solana_rpc_url);
    let payer = Keypair::from_base58_string(&config.payer_private_key);
    let mint_to_create_ata_for = Pubkey::from_str(MINT_ADDRESS)?;

    info!(payer = %payer.pubkey(), "Portefeuille Payeur");
    info!(mint = %mint_to_create_ata_for, "Mint du Jeton Cible");
    info!("Analyse du mint pour déterminer le programme de token...");
    let mint_account = rpc_client.get_account(&mint_to_create_ata_for).await
        .map_err(|e| anyhow!("Impossible de récupérer le compte du mint {}: {}. Assurez-vous d'être sur le bon cluster (Mainnet/Devnet).", mint_to_create_ata_for, e))?;

    let token_program_id = if mint_account.owner == spl_token_2022::id() {
        info!(mint_owner = %mint_account.owner, "Mint détecté comme appartenant au programme Token-2022.");

        spl_token_2022::id()
    } else if mint_account.owner == spl_token::id() {
        info!(mint_owner = %mint_account.owner, "Mint détecté comme appartenant au programme SPL Token standard.");

        spl_token::id()
    } else {
        bail!("Le propriétaire du mint ({}) n'est ni le programme SPL Token, ni Token-2022.", mint_account.owner);
    };

    // 3. Construire l'instruction de création d'ATA
    let create_ata_instruction = spl_associated_token_account::instruction::create_associated_token_account(
        &payer.pubkey(),
        &payer.pubkey(),
        &mint_to_create_ata_for,
        &token_program_id, // <-- ON UTILISE LE PROGRAMME DÉTECTÉ DYNAMIQUEMENT !
    );

    // 4. Créer, signer et envoyer la transaction
    let recent_blockhash = rpc_client.get_latest_blockhash().await?;
    let mut transaction = Transaction::new_with_payer(
        &[create_ata_instruction],
        Some(&payer.pubkey()),
    );
    transaction.sign(&[&payer], recent_blockhash);

    info!("Envoi de la transaction pour créer l'ATA...");

    let signature = rpc_client.send_and_confirm_transaction(&transaction).await?;

    // 5. Afficher le résultat
    let ata_address = spl_associated_token_account::get_associated_token_address(&payer.pubkey(), &mint_to_create_ata_for);

    info!(
    ata_address = %ata_address,
    signature = %signature,
    solscan_url = format!("https://solscan.io/tx/{}", signature),
    "✅ SUCCÈS ! L'ATA a été créé."
);

    Ok(())
}