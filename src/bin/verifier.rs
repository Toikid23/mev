// DANS : src/bin/verifier.rs - Version "Super-Vérificateur"

use anyhow::Result;
use solana_client::rpc_client::RpcClient;
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;
use std::collections::HashSet;

fn main() -> Result<()> {
    println!("--- Lancement du Super-Vérificateur de PDA ---");

    // --- Données de la transaction ---
    let pool_id_str = "YrrUStgPugDp8BbfosqDeFssen6sA75ZS1QJvgnHtmY";
    let program_id_str = "CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK";
    let rpc_url = "https://api.mainnet-beta.solana.com";

    let tick_current: i32 = 77738;
    let tick_spacing: u16 = 60;
    let ticks_per_array = 60 * tick_spacing as i32;

    let get_start_tick_index = |tick_index: i32| {
        let mut start = tick_index / ticks_per_array;
        if tick_index < 0 && tick_index % ticks_per_array != 0 {
            start -= 1;
        }
        start * ticks_per_array
    };

    // --- Initialisation ---
    let pool_id = Pubkey::from_str(pool_id_str)?;
    let program_id = Pubkey::from_str(program_id_str)?;
    let rpc_client = RpcClient::new(rpc_url.to_string());
    let active_array_start_index = get_start_tick_index(tick_current);

    // --- Calcul des adresses avec TOUTES les méthodes ---
    let mut all_possible_addresses = HashSet::new();
    println!("\n[1] Calcul des adresses PDA avec 3 méthodes différentes...");

    const WINDOW_SIZE: i32 = 5;
    for i in -WINDOW_SIZE..=WINDOW_SIZE {
        let start_tick = active_array_start_index + (i * ticks_per_array);

        // Méthode 1: Big-Endian (to_be_bytes)
        let (pda_be, _) = Pubkey::find_program_address(&[b"tick_array", &pool_id.to_bytes(), &start_tick.to_be_bytes()], &program_id);
        all_possible_addresses.insert(pda_be);

        // Méthode 2: Little-Endian (to_le_bytes)
        let (pda_le, _) = Pubkey::find_program_address(&[b"tick_array", &pool_id.to_bytes(), &start_tick.to_le_bytes()], &program_id);
        all_possible_addresses.insert(pda_le);

        // Méthode 3: String (to_string().as_bytes())
        let (pda_str, _) = Pubkey::find_program_address(&[b"tick_array", &pool_id.to_bytes(), start_tick.to_string().as_bytes()], &program_id);
        all_possible_addresses.insert(pda_str);
    }

    let addresses_to_fetch: Vec<Pubkey> = all_possible_addresses.into_iter().collect();
    println!(" -> {} adresses uniques générées pour vérification.", addresses_to_fetch.len());

    // --- Vérification avec la blockchain ---
    println!("\n[2] Lancement de l'appel RPC get_multiple_accounts...");
    let accounts_results = rpc_client.get_multiple_accounts(&addresses_to_fetch)?;
    println!(" -> Réponse RPC reçue.");

    // --- Analyse des résultats ---
    println!("\n[3] Analyse des comptes trouvés :");
    let mut found_addresses = HashSet::new();
    for (i, result) in accounts_results.iter().enumerate() {
        if result.is_some() {
            found_addresses.insert(addresses_to_fetch[i]);
            println!("  [TROUVÉ] {}", addresses_to_fetch[i]);
        }
    }

    // --- Conclusion ---
    println!("\n[4] CONCLUSION FINALE :");
    let mut a_winner_was_found = false;
    for i in -WINDOW_SIZE..=WINDOW_SIZE {
        let start_tick = active_array_start_index + (i * ticks_per_array);
        println!("\n--- Vérification pour start_tick = {} ---", start_tick);

        let (pda_be, _) = Pubkey::find_program_address(&[b"tick_array", &pool_id.to_bytes(), &start_tick.to_be_bytes()], &program_id);
        let (pda_le, _) = Pubkey::find_program_address(&[b"tick_array", &pool_id.to_bytes(), &start_tick.to_le_bytes()], &program_id);
        let (pda_str, _) = Pubkey::find_program_address(&[b"tick_array", &pool_id.to_bytes(), start_tick.to_string().as_bytes()], &program_id);

        if found_addresses.contains(&pda_be) {
            println!("  [!!! BINGO !!!] La méthode Big-Endian (to_be_bytes) a trouvé un compte : {}", pda_be);
            a_winner_was_found = true;
        } else {
            println!("  [ÉCHEC] Big-Endian: {}", pda_be);
        }
        if found_addresses.contains(&pda_le) {
            println!("  [!!! BINGO !!!] La méthode Little-Endian (to_le_bytes) a trouvé un compte : {}", pda_le);
            a_winner_was_found = true;
        } else {
            println!("  [ÉCHEC] Little-Endian: {}", pda_le);
        }
        if found_addresses.contains(&pda_str) {
            println!("  [!!! BINGO !!!] La méthode String (to_string) a trouvé un compte : {}", pda_str);
            a_winner_was_found = true;
        } else {
            println!("  [ÉCHEC] String: {}", pda_str);
        }
    }

    if !a_winner_was_found {
        println!("\n\nÉCHEC TOTAL : Aucune des méthodes testées n'a permis de trouver les comptes TickArray. Le problème est plus complexe.");
    } else {
        println!("\n\nSUCCÈS : La méthode correcte pour le calcul du PDA a été identifiée. Mettez à jour votre code en conséquence.");
    }

    Ok(())
}