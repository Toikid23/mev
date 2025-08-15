// DANS : src/bin/verify_strings.rs

use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;

fn main() {
    println!("--- Vérificateur de Chaînes d'Adresses ---");
    println!("Cet outil vérifie si les adresses codées en dur sont valides.\n");

    let addresses_to_check = vec![
        ("POOL_ADDRESS", "YrrUStgPugDp8BbfosqDeFssen6sA75ZS1QJvgnHtmY"),
        ("PROGRAM_ID", "CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK"),
        ("WSOL_MINT", "So11111111111111111111111111111111111111112"),
        ("LAUNCHCOIN_MINT", "Ey59PH7Z4BFU4HjyKnyMdWt5GGN76KazTAwQihoUXRnk"),
        ("TICK_ARRAY_1 (de la TX)", "3KgWL4nAvTBB5sW1hEe7NKuUZuCPQJiHfHskTT8AB7TD"),
        ("TICK_ARRAY_2 (de la TX)", "BxSVa5tjkUukFsJh8ruFFsjkmDUaLWxrE4dWewLdPxsES"),
    ];

    let mut all_ok = true;
    for (name, address_str) in addresses_to_check {
        match Pubkey::from_str(address_str) {
            Ok(pubkey) => {
                println!("✅ [OK] '{}' -> {}", name, pubkey);
            }
            Err(e) => {
                println!("❌ [ERREUR] Le test pour '{}' a ÉCHOUÉ.", name);
                println!("   L'erreur est : {}", e);
                println!("   La chaîne de caractères invalide est : \"{}\"", address_str);
                all_ok = false;
            }
        }
    }

    println!("\n--- Conclusion ---");
    if all_ok {
        println!("Toutes les adresses constantes dans cet outil sont valides.");
        println!("Si l'erreur persiste, cela signifie qu'une des chaînes dans `dev_runner.rs` a été corrompue lors d'un copier-coller.");
    } else {
        println!("Une ou plusieurs chaînes sont INVALIDE. Corrigez la chaîne marquée [ERREUR] dans votre code.");
    }
}