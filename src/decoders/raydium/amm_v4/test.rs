use anyhow::{anyhow, bail};
use solana_sdk::{
    pubkey::Pubkey,
    signature::Keypair,
    signer::Signer,
    transaction::VersionedTransaction,
    message::VersionedMessage,
};
use std::str::FromStr;
use spl_associated_token_account::get_associated_token_address;
use solana_client::rpc_config::RpcSimulateTransactionConfig;
use solana_transaction_status::UiTransactionEncoding;
use solana_account_decoder::UiAccountData;
use spl_token::state::Account as SplTokenAccount;
use solana_program_pack::Pack;
use crate::decoders::raydium::amm_v4::{
    decode_pool,
    hydrate,
    RAYDIUM_AMM_V4_PROGRAM_ID,
};
use crate::decoders::PoolOperations;
use crate::decoders::pool_operations::UserSwapAccounts;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use crate::rpc::ResilientRpcClient;


pub async fn test_ammv4_with_simulation(rpc_client: &ResilientRpcClient, payer_keypair: &Keypair, current_timestamp: i64) -> anyhow::Result<()> {
    const POOL_ADDRESS: &str = "58oQChx4yWmvKdwLLZzBi4ChoCc2fqCUWBkwMihLYQo2"; // WSOL-USDC
    const INPUT_MINT_STR: &str = "So11111111111111111111111111111111111111112"; // WSOL
    const INPUT_AMOUNT_UI: f64 = 0.05;

    println!("\n--- Test et Simulation (Lecture de Compte) Raydium AMMv4 ({}) ---", POOL_ADDRESS);
    let pool_pubkey = Pubkey::from_str(POOL_ADDRESS)?;

    // --- 1. Prédiction Locale (inchangée) ---
    println!("[1/3] Hydratation et prédiction locale...");
    let pool_account_data = rpc_client.get_account_data(&pool_pubkey).await?;
    let mut pool = decode_pool(&pool_pubkey, &pool_account_data)?;
    hydrate(&mut pool, rpc_client).await?;

    let input_mint_pubkey = Pubkey::from_str(INPUT_MINT_STR)?;
    let (input_decimals, output_decimals, output_mint_pubkey) = if input_mint_pubkey == pool.mint_a {
        (pool.mint_a_decimals, pool.mint_b_decimals, pool.mint_b)
    } else {
        (pool.mint_b_decimals, pool.mint_a_decimals, pool.mint_a)
    };
    let amount_in_base_units = (INPUT_AMOUNT_UI * 10f64.powi(input_decimals as i32)) as u64;

    let predicted_amount_out = pool.get_quote(&input_mint_pubkey, amount_in_base_units, current_timestamp)?;
    let ui_predicted_amount_out = predicted_amount_out as f64 / 10f64.powi(output_decimals as i32);
    println!("-> PRÉDICTION LOCALE: {}", ui_predicted_amount_out);


    println!("\n[VALIDATION] Lancement du test d'inversion mathématique...");
    if predicted_amount_out > 0 {
        let required_input_from_quote = pool.get_required_input(&output_mint_pubkey, predicted_amount_out, current_timestamp)?;
        println!("  -> Input original     : {}", amount_in_base_units);
        println!("  -> Output prédit      : {}", predicted_amount_out);
        println!("  -> Input Re-calculé   : {}", required_input_from_quote);

        // On vérifie que le résultat est égal ou légèrement supérieur.
        if required_input_from_quote >= amount_in_base_units {
            let difference = required_input_from_quote.saturating_sub(amount_in_base_units);
            // On tolère une petite différence due aux arrondis successifs.
            if difference <= 10 { // 10 lamports est une tolérance très acceptable
                println!("  -> ✅ SUCCÈS : Le calcul inverse est cohérent (différence: {} lamports).", difference);
            } else {
                bail!("  -> ⚠️ ÉCHEC : La différence du calcul inverse est trop grande ({} lamports).", difference);
            }
        } else {
            bail!("  -> ⚠️ ÉCHEC : Le calcul inverse a produit un montant inférieur à l'original !");
        }
    } else {
        println!("  -> AVERTISSEMENT : Le quote est de 0, test d'inversion sauté.");
    }


    // --- 2. Préparation de la Transaction (inchangée) ---
    println!("\n[2/3] Préparation de la transaction de swap...");
    let payer_pubkey = payer_keypair.pubkey();
    let user_source_ata = get_associated_token_address(&payer_pubkey, &input_mint_pubkey);
    let user_destination_ata = get_associated_token_address(&payer_pubkey, &output_mint_pubkey);


    // --- NOUVEAU: Pré-vérification de tous les comptes ---
    println!("\n[DIAGNOSTIC] Vérification de l'existence de tous les comptes requis...");
    let (amm_authority, _) = Pubkey::find_program_address(&[b"amm authority"], &RAYDIUM_AMM_V4_PROGRAM_ID);

    // On recrée la liste EXACTE des clés qui seront utilisées dans l'instruction
    let account_keys_to_check = vec![
        spl_token::id(),
        pool.address,
        amm_authority,
        pool.open_orders,
        pool.target_orders,
        pool.vault_a,
        pool.vault_b,
        pool.market_program_id,
        pool.market,
        pool.market_bids,
        pool.market_asks,
        pool.market_event_queue,
        pool.market_coin_vault,
        pool.market_pc_vault,
        pool.market_vault_signer,
        user_source_ata,
        user_destination_ata,
        payer_pubkey,
    ];

    let account_results = rpc_client.get_multiple_accounts(&account_keys_to_check).await?;

    let mut all_found = true;
    for (i, result) in account_results.iter().enumerate() {
        let pubkey = account_keys_to_check[i];
        if result.is_some() {
            println!("  -> Compte {}: {} ... Trouvé ✅", i, pubkey);
        } else {
            println!("  -> Compte {}: {} ... MANQUANT ❌", i, pubkey);
            all_found = false;
        }
    }

    if !all_found {
        bail!("Vérification échouée : Un ou plusieurs comptes requis pour la transaction n'existent pas sur le RPC interrogé.");
    }
    println!("[DIAGNOSTIC] Tous les comptes ont été trouvés. Le problème est ailleurs.");
    // --- FIN DU BLOC DE DIAGNOSTIC ---


    let mut instructions = Vec::new();
    if rpc_client.get_account(&user_destination_ata).await.is_err() {
        instructions.push(
            spl_associated_token_account::instruction::create_associated_token_account(
                &payer_pubkey, &payer_pubkey, &output_mint_pubkey, &spl_token::id(),
            ),
        );
    }

    // Étape 1: Regrouper les comptes utilisateur dans la struct
    let user_accounts = UserSwapAccounts {
        owner: payer_pubkey,
        source: user_source_ata,
        destination: user_destination_ata,
    };

    // Étape 2: Appeler la nouvelle fonction avec la bonne signature
    let swap_ix = pool.create_swap_instruction(
        &input_mint_pubkey,      // Arg 1: token_in_mint
        amount_in_base_units,    // Arg 2: amount_in
        0,                       // Arg 3: min_amount_out
        &user_accounts,          // Arg 4: user_accounts
    )?;


    instructions.push(swap_ix);

    let recent_blockhash = rpc_client.get_latest_blockhash().await?;
    let transaction = VersionedTransaction::try_new(
        VersionedMessage::V0(solana_sdk::message::v0::Message::try_compile(
            &payer_pubkey,
            &instructions,
            &[], // Pas de LUT pour ce simple test
            recent_blockhash,
        )?),
        &[payer_keypair],
    )?;

    // --- 3. Simulation et Analyse par Lecture de Compte ---
    println!("\n[3/3] Exécution de la simulation avec lecture de compte...");

    // On récupère la balance du compte de destination AVANT la simulation.
    let initial_destination_balance = match rpc_client.get_account(&user_destination_ata).await {
        Ok(acc) => SplTokenAccount::unpack(&acc.data)?.amount,
        Err(_) => 0,
    };
    println!("-> Balance initiale du compte de destination: {}", initial_destination_balance);

    // Configuration de la simulation pour qu'elle nous retourne l'état du compte de destination.
    let sim_config = RpcSimulateTransactionConfig {
        sig_verify: false,
        replace_recent_blockhash: true,
        commitment: Some(rpc_client.commitment()),
        encoding: Some(UiTransactionEncoding::Base64),
        accounts: Some(solana_client::rpc_config::RpcSimulateTransactionAccountsConfig {
            encoding: Some(solana_account_decoder::UiAccountEncoding::Base64),
            addresses: vec![ user_destination_ata.to_string() ],
        }),
        ..Default::default()
    };

    let sim_response = rpc_client.simulate_transaction_with_config(&transaction, sim_config).await?;
    let sim_result = sim_response.value;

    if let Some(err) = sim_result.err {
        println!("!! ERREUR DE SIMULATION: {:?}", err);
        println!("!! LOGS: {:?}", sim_result.logs);
        bail!("La simulation a échoué.");
    }

    // Extraction de la balance POST-simulation
    let post_accounts = sim_result.accounts.ok_or_else(|| anyhow!("La simulation n'a pas retourné l'état des comptes."))?;
    let destination_account_state = post_accounts.first().and_then(|acc| acc.as_ref()).ok_or_else(|| anyhow!("L'état du compte de destination n'a pas été retourné."))?;

    let decoded_data = match &destination_account_state.data {
        UiAccountData::Binary(data_str, _) => STANDARD.decode(data_str)?,
        _ => bail!("Format de données de compte inattendu."),
    };

    let token_account_data = SplTokenAccount::unpack(&decoded_data)?;
    let post_simulation_balance = token_account_data.amount;
    println!("-> Balance finale du compte de destination (simulée): {}", post_simulation_balance);

    // Le montant reçu est simplement la différence.
    let simulated_amount_out = post_simulation_balance.saturating_sub(initial_destination_balance);
    let ui_simulated_amount_out = simulated_amount_out as f64 / 10f64.powi(output_decimals as i32);

    // --- 4. Comparaison ---
    println!("\n--- COMPARAISON (AMMv4) ---");
    println!("Montant PRÉDIT (local)             : {}", ui_predicted_amount_out);
    println!("Montant SIMULÉ (lecture de compte) : {}", ui_simulated_amount_out);
    let difference = (ui_predicted_amount_out - ui_simulated_amount_out).abs();
    let difference_percent = if ui_simulated_amount_out > 0.0 { (difference / ui_simulated_amount_out) * 100.0 } else { 0.0 };
    println!("-> Différence relative: {:.6} %", difference_percent);

    if difference_percent < 0.1 {
        println!("✅ SUCCÈS ! Le décodeur Raydium AMMv4 est précis.");
    } else {
        println!("⚠️ ÉCHEC ! La différence est trop grande.");
    }

    Ok(())
}