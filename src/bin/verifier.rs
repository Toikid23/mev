// DANS : src/bin/verifier.rs

use anyhow::Result;
use solana_client::rpc_client::RpcClient;
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;

// La fonction de calcul de PDA qui a prouvé qu'elle fonctionnait pour start_tick = 0.
// Nous nous engageons à n'utiliser que celle-ci.
fn get_tick_array_address(pool_id: &Pubkey, start_tick_index: i32, program_id: &Pubkey) -> Pubkey {
    let (pda, _) = Pubkey::find_program_address(
        &[
            b"tick_array",
            &pool_id.to_bytes(),
            &start_tick_index.to_be_bytes(), // <--- LA CORRECTION
        ],
        program_id,
    );
    pda
}

fn main() -> Result<()> {
    println!("--- Test Final de la Fonction de Calcul d'Adresse ---");

    // --- Données de laboratoire ---
    let pool_id_str = "3ucNos4NbumPLZNWztqGHNFFgkHeRMBQAVemeeomsUxv";
    let program_id_str = "CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK";
    // REMPLACEZ CECI PAR VOTRE URL RPC
    let rpc_url = "https://api.mainnet-beta.solana.com";

    let tick_current: i32 = -16832;
    let tick_spacing: u16 = 1;
    let ticks_per_array = 60 * tick_spacing as i32;
    // La fonction qui calcule le start_tick de l'array
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

    // --- Calcul des adresses ---
    let mut addresses_to_fetch = Vec::new();
    const WINDOW_SIZE: i32 = 3;
    for i in -WINDOW_SIZE..=WINDOW_SIZE {
        let target_start_index = active_array_start_index + (i * ticks_per_array);
        let pda = get_tick_array_address(&pool_id, target_start_index, &program_id);
        addresses_to_fetch.push(pda);
    }

    // --- Vérification avec la blockchain ---
    println!("\n[1] Lancement de l'appel RPC get_multiple_accounts pour la fenêtre...");
    let accounts_results = rpc_client.get_multiple_accounts(&addresses_to_fetch)?;
    println!(" -> Réponse RPC reçue.");

    println!("\n[2] Vérification des résultats :");
    println!("----------------------------------------------------------------------------------");
    let mut all_found = true;
    for (i, result) in accounts_results.iter().enumerate() {
        let address = addresses_to_fetch[i];
        if result.is_some() {
            println!("  [OK]    Adresse {} : Compte TROUVÉ", address);
        } else {
            println!("  [ÉCHEC] Adresse {} : Compte NON TROUVÉ", address);
            all_found = false;
        }
    }
    println!("----------------------------------------------------------------------------------");

    if all_found {
        println!("\nPREUVE FINALE : Le problème est résolu. La fonction de calcul PDA est correcte.");
    } else {
        println!("\nÉCHEC : Le problème persiste, il est plus profond que le calcul d'adresse.");
    }

    Ok(())
}