// DANS : src/bin/lut_analyzer.rs

use anyhow::Result;
use mev::{config::Config, rpc::ResilientRpcClient};
use solana_address_lookup_table_program::state::AddressLookupTable;
use solana_sdk::{
    pubkey::Pubkey, signature::{Keypair, Signature}, signer::Signer,
};
use std::{
    collections::{HashMap, HashSet},
    fs::File,
    io::Write,
    str::FromStr,
};
use solana_transaction_status::{UiTransactionEncoding, UiLoadedAddresses};

const MANAGED_LUT_ADDRESS: &str = "E5h798UBdK8V1L7MvRfi1ppr2vitPUUUUCVqvTyDgKXN";
const TX_HISTORY_LIMIT: usize = 1000;
const ANALYSIS_OUTPUT_FILE: &str = "lut_analysis_report.txt";
const SUGGESTIONS_FILE: &str = "lut_addresses_to_add.txt";

#[tokio::main]
async fn main() -> Result<()> {
    println!("--- Lancement de l'Analyseur de LUT (Mode Rapport Uniquement) ---");
    let config = Config::load()?;
    let rpc_client = ResilientRpcClient::new(config.solana_rpc_url, 5, 500);
    let payer = Keypair::from_base58_string(&config.payer_private_key);
    let lut_pubkey = Pubkey::from_str(MANAGED_LUT_ADDRESS)?;

    println!("Portefeuille analysé : {}", payer.pubkey());
    println!("LUT cible : {}", lut_pubkey);

    let on_chain_addresses = match rpc_client.get_account(&lut_pubkey).await {
        Ok(acc) => {
            let lut_data = AddressLookupTable::deserialize(&acc.data)?;
            println!("  -> {} adresses trouvées dans la LUT.", lut_data.addresses.len());
            // <-- LA CORRECTION EST ICI
            // On clone chaque Pubkey pour en prendre possession.
            // .cloned() est un raccourci pour .map(|pk| pk.clone())
            lut_data.addresses.iter().cloned().collect::<HashSet<_>>()
        }
        Err(_) => {
            println!("  -> AVERTISSEMENT: La LUT n'existe pas ou est inaccessible.");
            HashSet::new()
        }
    };

    println!("\n[2/4] Récupération de l'historique des transactions...");
    let signatures_with_status = rpc_client
        .get_signatures_for_address_with_limit(&payer.pubkey(), TX_HISTORY_LIMIT)
        .await?;
    println!("  -> {} signatures à analyser...", signatures_with_status.len());

    let mut address_counts: HashMap<Pubkey, usize> = HashMap::new();
    let encoding_config = Some(UiTransactionEncoding::Base64);

    for tx_status in signatures_with_status {
        let signature = Signature::from_str(&tx_status.signature)?;

        if let Ok(tx_with_meta) = rpc_client.get_transaction(&signature, encoding_config).await {
            // Logique pour les comptes statiques
            if let Some(decoded_tx) = tx_with_meta.transaction.transaction.decode() {
                let static_account_keys = decoded_tx.message.static_account_keys();
                for key in static_account_keys {
                    *address_counts.entry(*key).or_insert(0) += 1;
                }
            }

            // Logique pour les comptes chargés via LUT
            if let Some(meta) = tx_with_meta.transaction.meta {
                let loaded_addresses_opt: Option<UiLoadedAddresses> = meta.loaded_addresses.into();
                if let Some(loaded_addresses) = loaded_addresses_opt {
                    // Les adresses ici sont des Strings, il faut les convertir.
                    for key_str in loaded_addresses.writable {
                        if let Ok(key_pubkey) = Pubkey::from_str(&key_str) {
                            *address_counts.entry(key_pubkey).or_insert(0) += 1;
                        }
                    }
                    for key_str in loaded_addresses.readonly {
                        if let Ok(key_pubkey) = Pubkey::from_str(&key_str) {
                            *address_counts.entry(key_pubkey).or_insert(0) += 1;
                        }
                    }
                }
            }
        }
    }

    println!("\n[3/4] Filtrage et tri des adresses par fréquence...");
    let common_addresses: HashSet<Pubkey> = [
        spl_token::ID,
        spl_associated_token_account::ID,
        solana_sdk::system_program::ID,
        solana_sdk::compute_budget::ID,
        payer.pubkey(),
        lut_pubkey,
    ]
        .iter()
        .cloned()
        .collect();

    let mut sorted_addresses: Vec<(Pubkey, usize)> = address_counts
        .into_iter()
        .filter(|(key, _)| !common_addresses.contains(key))
        .collect();
    sorted_addresses.sort_by(|a, b| b.1.cmp(&a.1));

    println!("\n[4/4] Génération des rapports...");
    let mut report_content = String::new();
    report_content.push_str("--- Rapport d'Analyse de la LUT ---\n\n");
    report_content.push_str(&format!("LUT Adresse: {}\n", lut_pubkey));
    report_content.push_str(&format!("Adresses actuellement on-chain: {}\n\n", on_chain_addresses.len()));
    report_content.push_str("Fréquence des adresses dans les dernières transactions :\n");
    report_content.push_str("----------------------------------------------------------\n");
    report_content.push_str("Status     | Fréquence | Adresse\n");
    report_content.push_str("----------------------------------------------------------\n");

    let mut suggestions_content = String::new();
    for (address, count) in sorted_addresses.iter().take(256) {
        let status = if on_chain_addresses.contains(address) {
            " DÉJÀ INCLUS "
        } else {
            suggestions_content.push_str(&address.to_string());
            suggestions_content.push('\n');
            "**À AJOUTER**"
        };
        report_content.push_str(&format!(
            "{} | {:<10}| {}\n",
            status,
            count,
            address
        ));
    }

    let mut report_file = File::create(ANALYSIS_OUTPUT_FILE)?;
    report_file.write_all(report_content.as_bytes())?;

    let mut suggestions_file = File::create(SUGGESTIONS_FILE)?;
    suggestions_file.write_all(suggestions_content.as_bytes())?;

    println!("\n✅ --- ANALYSE TERMINÉE --- ✅");
    println!("  -> Rapport complet sauvegardé dans : {}", ANALYSIS_OUTPUT_FILE);
    println!("  -> Suggestions d'adresses à ajouter sauvegardées dans : {}", SUGGESTIONS_FILE);
    println!("\nACTION REQUISE : Vérifiez `{}` et utilisez `lut_manager` pour appliquer les changements.", SUGGESTIONS_FILE);

    Ok(())
}