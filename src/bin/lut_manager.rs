#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

use anyhow::{Context, Result};
use mev::{config::Config, rpc::ResilientRpcClient};
use solana_address_lookup_table_program::instruction;
use solana_sdk::{
    pubkey::Pubkey, signature::Keypair, signer::Signer,
    transaction::VersionedTransaction,
    message::{v0, VersionedMessage},
};
use std::{fs, str::FromStr};

// L'adresse de VOTRE LUT. C'est la source de vérité.
const MANAGED_LUT_ADDRESS: &str = "E5h798UBdK8V1L7MvRfi1ppr2vitPUUUUCVqvTyDgKXN"; // Remplacez par votre adresse de LUT
const ADDRESSES_TO_ADD_FILE: &str = "lut_addresses_to_add.txt";

#[tokio::main]
async fn main() -> Result<()> {
    println!("--- Lancement du Gestionnaire de LUT (Mode Écriture) ---");
    let config = Config::load()?;
    let rpc_client = ResilientRpcClient::new(config.solana_rpc_url, 3, 500);
    let payer = Keypair::from_base58_string(&config.payer_private_key);
    let lut_pubkey = Pubkey::from_str(MANAGED_LUT_ADDRESS)?;

    println!("Autorité : {}", payer.pubkey());
    println!("LUT cible : {}", lut_pubkey);

    // Lire les adresses à ajouter depuis le fichier que VOUS avez préparé.
    let file_content = fs::read_to_string(ADDRESSES_TO_ADD_FILE)
        .context(format!("Impossible de lire le fichier '{}'", ADDRESSES_TO_ADD_FILE))?;

    let addresses_to_add: Vec<Pubkey> = file_content
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(Pubkey::from_str)
        .collect::<Result<Vec<_>, _>>()?;

    if addresses_to_add.is_empty() {
        println!("Le fichier '{}' est vide. Aucune action à effectuer.", ADDRESSES_TO_ADD_FILE);
        return Ok(());
    }

    println!("{} adresses seront ajoutées à la LUT.", addresses_to_add.len());

    let mut instructions = Vec::new();
    for chunk in addresses_to_add.chunks(30) {
        let extend_ix = instruction::extend_lookup_table(
            lut_pubkey,
            payer.pubkey(),
            Some(payer.pubkey()),
            chunk.to_vec(),
        );
        instructions.push(extend_ix);
    }

    println!("Envoi de {} transaction(s) pour étendre la LUT...", instructions.len());
    let recent_blockhash = rpc_client.get_latest_blockhash().await?;

    for ix in instructions {
        let message = v0::Message::try_compile(&payer.pubkey(), &[ix], &[], recent_blockhash)?;
        let tx = VersionedTransaction::try_new(VersionedMessage::V0(message), &[&payer])?;
        let signature = rpc_client.send_and_confirm_transaction(&tx).await?;
        println!("  -> Extension réussie. Signature: {}", signature);
    }

    println!("\n✅ --- LUT MISE À JOUR AVEC SUCCÈS --- ✅");
    Ok(())
}