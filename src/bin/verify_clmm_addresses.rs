// DANS : src/bin/verify_clmm_addresses.rs

use anyhow::{Result, bail};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;
use std::collections::HashMap;

use mev::{
    config::Config,
    decoders::raydium_decoders::{clmm_pool},
};

// Fonction pour vérifier un ensemble de comptes et afficher leur statut
async fn check_accounts(
    rpc_client: &RpcClient,
    accounts_to_check: HashMap<String, Pubkey>
) -> Result<bool> {
    let mut all_ok = true;
    let pubkeys: Vec<Pubkey> = accounts_to_check.values().cloned().collect();

    println!("\n -> Vérification de {} comptes...", pubkeys.len());

    let results = rpc_client.get_multiple_accounts(&pubkeys).await?;

    for (i, (name, pubkey)) in accounts_to_check.iter().enumerate() {
        match results[i] {
            Some(_) => {
                println!("    ✅ [OK] Le compte '{}' ({}) a été trouvé.", name, pubkey);
            }
            None => {
                println!("    ❌ [ERREUR] Le compte '{}' ({}) est INTROUVABLE.", name, pubkey);
                all_ok = false;
            }
        }
    }
    Ok(all_ok)
}


#[tokio::main]
async fn main() -> Result<()> {
    println!("--- Lancement du VÉRIFICATEUR D'ADRESSES CLMM ---");
    let config = Config::load()?;
    let rpc_client = RpcClient::new(config.solana_rpc_url);

    // --- SETUP DU TEST CLMM (identique à dev_runner) ---
    const POOL_ADDRESS: &str = "YrrUStgPugDp8BbfosqDeFssen6sA75ZS1QJvgnHtmY";
    const PROGRAM_ID: &str = "CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK";
    const INPUT_MINT_STR: &str = "So11111111111111111111111111111111111111112";
    let pool_pubkey = Pubkey::from_str(POOL_ADDRESS)?;
    let program_pubkey = Pubkey::from_str(PROGRAM_ID)?;
    let input_mint_pubkey = Pubkey::from_str(INPUT_MINT_STR)?;

    // --- 1. Hydratation pour obtenir toutes les adresses dérivées ---
    println!("\n[1/3] Hydratation du pool CLMM...");
    let pool_account_data = rpc_client.get_account_data(&pool_pubkey).await?;
    let mut pool = clmm_pool::decode_pool(&pool_pubkey, &pool_account_data, &program_pubkey)?;
    clmm_pool::hydrate(&mut pool, &rpc_client).await?;
    println!("-> Hydratation terminée.");

    // --- 2. Construction de l'instruction pour obtenir la liste de comptes exacte ---
    println!("\n[2/3] Construction de l'instruction de swap...");
    // On simule un utilisateur et un montant pour construire l'instruction
    let temp_user = Pubkey::new_unique();
    let temp_source_ata = spl_associated_token_account::get_associated_token_address(&temp_user, &pool.mint_a);
    let temp_dest_ata = spl_associated_token_account::get_associated_token_address(&temp_user, &pool.mint_b);
    const INPUT_AMOUNT_UI: f64 = 0.1;
    let amount_in_base_units = (INPUT_AMOUNT_UI * 10f64.powi(pool.mint_a_decimals as i32)) as u64;


    // On utilise la simulation locale pour savoir quels tick arrays sont nécessaires
    let (_, required_tick_arrays) = pool.get_quote_with_tick_arrays(&input_mint_pubkey, amount_in_base_units, 0)?;

    let swap_ix = pool.create_swap_instruction(
        &input_mint_pubkey,
        &temp_source_ata,
        &temp_dest_ata,
        &temp_user,
        amount_in_base_units, 0,
        required_tick_arrays
    )?;
    println!("-> Instruction construite. Elle requiert {} comptes.", swap_ix.accounts.len());

    // --- 3. Vérification de CHAQUE compte ---
    let mut accounts_to_verify = HashMap::new();
    for (i, meta) in swap_ix.accounts.iter().enumerate() {
        // On donne un nom générique à chaque compte pour l'affichage
        accounts_to_verify.insert(format!("Compte #{}", i), meta.pubkey);
    }

    let all_found = check_accounts(&rpc_client, accounts_to_verify).await?;

    println!("\n--- CONCLUSION ---");
    if all_found {
        println!("✅ SUCCÈS ! TOUS les comptes requis par l'instruction existent.");
        println!("Si l'erreur persiste, le problème vient peut-être du contexte de simulation et non des adresses.");
    } else {
        println!("❌ ÉCHEC ! Au moins un compte requis par l'instruction est INTROUVABLE.");
        println!("L'adresse marquée [ERREUR] ci-dessus est la cause du problème. Corrigez sa logique de dérivation.");
    }

    Ok(())
}