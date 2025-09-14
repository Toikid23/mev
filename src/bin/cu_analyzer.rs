// DANS : src/bin/cu_analyzer.rs

use anyhow::{anyhow, Result};
use mev::config::Config;
use mev::decoders::pool_operations::{PoolOperations, UserSwapAccounts};
use mev::decoders::{orca, pump, raydium, meteora, Pool};
use mev::rpc::ResilientRpcClient;
use solana_sdk::{
    message::VersionedMessage, pubkey::Pubkey, signature::Keypair, signer::Signer,
    transaction::VersionedTransaction,
};
use spl_associated_token_account::get_associated_token_address_with_program_id;
use solana_client::rpc_config::RpcSimulateTransactionConfig;
use solana_transaction_status::UiTransactionEncoding;
use spl_associated_token_account::get_associated_token_address;
use std::str::FromStr;

/// Structure pour stocker le résultat d'une analyse.
struct AnalysisResult {
    name: String,
    result: Result<u64>,
}

/// Exécute un swap via une simulation et retourne les CUs consommés.
async fn execute_and_get_cus(
    rpc_client: &ResilientRpcClient,
    payer: &Keypair,
    pool: Pool,
    token_in_mint: Pubkey,
    amount_in: u64,
) -> Result<u64> {
    let (mint_a, mint_b) = pool.get_mints();
    let token_out_mint = if token_in_mint == mint_a { mint_b } else { mint_a };

    // <-- BLOC DE LOGIQUE COMPLET ET FINAL
    // On récupère les program_id des mints pour dériver les bonnes adresses ATA.
    let (input_token_program, output_token_program) = match &pool {
        Pool::RaydiumClmm(p) if token_in_mint == p.mint_a => (p.mint_a_program, p.mint_b_program),
        Pool::RaydiumClmm(p) => (p.mint_b_program, p.mint_a_program),

        Pool::OrcaWhirlpool(p) if token_in_mint == p.mint_a => (p.mint_a_program, p.mint_b_program),
        Pool::OrcaWhirlpool(p) => (p.mint_b_program, p.mint_a_program),

        Pool::MeteoraDlmm(p) if token_in_mint == p.mint_a => (p.mint_a_program, p.mint_b_program),
        Pool::MeteoraDlmm(p) => (p.mint_b_program, p.mint_a_program),

        Pool::MeteoraDammV2(p) if token_in_mint == p.mint_a => (p.mint_a_program, p.mint_b_program),
        Pool::MeteoraDammV2(p) => (p.mint_b_program, p.mint_a_program),

        Pool::RaydiumCpmm(p) if token_in_mint == p.token_0_mint => (p.token_0_program, p.token_1_program),
        Pool::RaydiumCpmm(p) => (p.token_1_program, p.token_0_program),

        Pool::PumpAmm(p) if token_in_mint == p.mint_a => (p.mint_a_program, p.mint_b_program),
        Pool::PumpAmm(p) => (p.mint_b_program, p.mint_a_program),

        // Fallback pour les pools qui n'ont pas de champs de programme de token spécifiques (ex: Raydium AMMv4)
        _ => (spl_token::ID, spl_token::ID),
    };

    let user_accounts = UserSwapAccounts {
        owner: payer.pubkey(),
        source: get_associated_token_address_with_program_id(&payer.pubkey(), &token_in_mint, &input_token_program),
        destination: get_associated_token_address_with_program_id(&payer.pubkey(), &token_out_mint, &output_token_program),
    };

    let swap_ix = pool.create_swap_instruction(&token_in_mint, amount_in, 1, &user_accounts)?;

    // <-- NOUVEAU BLOC DE CRÉATION D'ATA
    let mut instructions = Vec::new();
    // On vérifie si l'ATA de destination existe
    if rpc_client.get_account(&user_accounts.destination).await.is_err() {
        println!("\n    -> ATA de destination {} non trouvé. Ajout de l'instruction de création.", user_accounts.destination);
        instructions.push(
            spl_associated_token_account::instruction::create_associated_token_account(
                &payer.pubkey(),
                &payer.pubkey(),
                &token_out_mint,
                // On utilise le bon programme de token ! Crucial pour Token-2022.
                &output_token_program,
            ),
        );
    }
    instructions.push(swap_ix);
    // --- FIN DU BLOC

    let recent_blockhash = rpc_client.get_latest_blockhash().await?;
    let message = VersionedMessage::V0(solana_sdk::message::v0::Message::try_compile(
        &payer.pubkey(),
        &instructions, // <-- On utilise le vecteur d'instructions
        &[],
        recent_blockhash,
    )?);
    let transaction = VersionedTransaction::try_new(message, &[payer])?;

    // <-- MODIFIÉ : On ajoute le champ manquant
    let sim_config = RpcSimulateTransactionConfig {
        sig_verify: false,
        replace_recent_blockhash: true,
        commitment: Some(rpc_client.commitment()),
        encoding: Some(UiTransactionEncoding::Base64),
        accounts: None,
        min_context_slot: None,
        inner_instructions: false, // <-- Le champ manquant
    };

    let sim_result = rpc_client
        .simulate_transaction_with_config(&transaction, sim_config)
        .await?
        .value;

    if let Some(err) = sim_result.err {
        println!("\nLogs pour l'erreur: {:?}", sim_result.logs);
        return Err(anyhow!("La simulation a échoué: {:?}", err));
    }

    sim_result
        .units_consumed
        .ok_or_else(|| anyhow!("Units consumed non disponible dans la simulation."))
}

// <-- NOUVEAU : Fonctions libres au lieu de l'impl
fn get_mint_a_decimals(pool: &Pool) -> u8 {
    match pool {
        Pool::RaydiumAmmV4(p) => p.mint_a_decimals,
        Pool::RaydiumCpmm(p) => p.mint_0_decimals,
        Pool::RaydiumClmm(p) => p.mint_a_decimals,
        Pool::MeteoraDammV1(p) => p.mint_a_decimals,
        Pool::MeteoraDammV2(p) => p.mint_a_decimals,
        Pool::MeteoraDlmm(p) => p.mint_a_decimals,
        Pool::OrcaWhirlpool(p) => p.mint_a_decimals,
        Pool::PumpAmm(p) => p.mint_a_decoded.decimals,
    }
}

fn get_mint_b_decimals(pool: &Pool) -> u8 {
    match pool {
        Pool::RaydiumAmmV4(p) => p.mint_b_decimals,
        Pool::RaydiumCpmm(p) => p.mint_1_decimals,
        Pool::RaydiumClmm(p) => p.mint_b_decimals,
        Pool::MeteoraDammV1(p) => p.mint_b_decimals,
        Pool::MeteoraDammV2(p) => p.mint_b_decimals,
        Pool::MeteoraDlmm(p) => p.mint_b_decimals,
        Pool::OrcaWhirlpool(p) => p.mint_b_decimals,
        Pool::PumpAmm(p) => p.mint_b_decoded.decimals,
    }
}

/// Charge, hydrate un pool et lance l'analyse.
async fn analyze_pool(
    rpc_client: &ResilientRpcClient,
    payer: &Keypair,
    pool_address: &str,
    input_mint_address: &str,
    amount_in_ui: f64,
) -> Result<u64> {
    let pool_pubkey = Pubkey::from_str(pool_address)?;
    let input_mint_pubkey = Pubkey::from_str(input_mint_address)?;
    let account = rpc_client.get_account(&pool_pubkey).await?;

    let raw_pool = match account.owner {
        id if id == raydium::amm_v4::RAYDIUM_AMM_V4_PROGRAM_ID => raydium::amm_v4::decode_pool(&pool_pubkey, &account.data).map(Pool::RaydiumAmmV4),
        id if id == Pubkey::from_str("CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C").unwrap() => raydium::cpmm::decode_pool(&pool_pubkey, &account.data).map(Pool::RaydiumCpmm),
        id if id == Pubkey::from_str("CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK").unwrap() => raydium::clmm::decode_pool(&pool_pubkey, &account.data, &id).map(Pool::RaydiumClmm),
        id if id == Pubkey::from_str("Eo7WjKq67rjJQSZxS6z3YkapzY3eMj6Xy8X5EQVn5UaB").unwrap() => meteora::damm_v1::decode_pool(&pool_pubkey, &account.data).map(Pool::MeteoraDammV1),
        id if id == Pubkey::from_str("cpamdpZCGKUy5JxQXB4dcpGPiikHawvSWAd6mEn1sGG").unwrap() => meteora::damm_v2::decode_pool(&pool_pubkey, &account.data).map(Pool::MeteoraDammV2),
        id if id == Pubkey::from_str("LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo").unwrap() => meteora::dlmm::decode_lb_pair(&pool_pubkey, &account.data, &account.owner).map(Pool::MeteoraDlmm),
        id if id == Pubkey::from_str("whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc").unwrap() => orca::whirlpool::decode_pool(&pool_pubkey, &account.data).map(Pool::OrcaWhirlpool),
        id if id == pump::amm::PUMP_PROGRAM_ID => pump::amm::decode_pool(&pool_pubkey, &account.data).map(Pool::PumpAmm),
        _ => Err(anyhow!("Programme propriétaire inconnu: {}", account.owner)),
    }?;

    let hydrated_pool = mev::graph_engine::Graph::hydrate_pool(raw_pool, rpc_client).await?;

    let (mint_a, mint_b) = hydrated_pool.get_mints();

    // <-- MODIFIÉ : On appelle nos nouvelles fonctions libres
    let decimals = if input_mint_pubkey == mint_a {
        get_mint_a_decimals(&hydrated_pool)
    } else if input_mint_pubkey == mint_b {
        get_mint_b_decimals(&hydrated_pool)
    } else {
        return Err(anyhow!("Le mint d'input ne correspond à aucun mint du pool"));
    };

    let amount_in_base = (amount_in_ui * 10f64.powi(decimals as i32)) as u64;

    execute_and_get_cus(rpc_client, payer, hydrated_pool, input_mint_pubkey, amount_in_base).await
}


#[tokio::main]
async fn main() -> Result<()> {
    // ... (contenu de la fonction main inchangé)
    println!("--- Lancement de l'Analyseur de Compute Units ---");
    let config = Config::load()?;
    let rpc_client = ResilientRpcClient::new(config.solana_rpc_url, 3, 500);
    let payer = Keypair::from_base58_string(&config.payer_private_key);
    println!("Utilisation du portefeuille : {}", payer.pubkey());

    let wsol_mint = "So11111111111111111111111111111111111111112";

    let pools_to_test = vec![
        ("Raydium AMM V4", "58oQChx4yWmvKdwLLZzBi4ChoCc2fqCUWBkwMihLYQo2", wsol_mint, 0.01),
        ("Raydium CPMM", "8ujpQXxnnWvRohU2oCe3eaSzoL7paU2uj3fEn4Zp72US", wsol_mint, 0.01),
        ("Raydium CLMM", "YrrUStgPugDp8BbfosqDeFssen6sA75ZS1QJvgnHtmY", wsol_mint, 0.01),
        ("Orca Whirlpool", "Czfq3xZZDmsdGdUyrNLtRhGc47cXcZtLG4crryfu44zE", wsol_mint, 0.01),
        ("Meteora DAMM V1", "5rCf1DM8LjKTw4YqhnoLcngyZYeNnQqztScTogYHAS6", wsol_mint, 0.01),
        ("Meteora DLMM", "GcnHKJgMxeUCy7PUcVEssZ6swiAUt9KFPky3EjSLJL3f", wsol_mint, 0.01),
        ("Meteora DAMM V2", "FiMTgvjJq7dWX5ZetXZv6XeHxXSYJRG2SgNR9mygs9KN", wsol_mint, 0.01),
        ("Pump AMM", "CLYFHhJfJjNPSMQv7byFeAsZ8x1EXQyYkGTPrNc2vc78", wsol_mint, 0.01),
    ];

    let mut results = Vec::new();
    for (name, pool_address, input_mint, amount_in) in pools_to_test {
        let result = analyze_pool(&rpc_client, &payer, pool_address, input_mint, amount_in).await;
        results.push(AnalysisResult { name: name.to_string(), result });
    }

    println!("\n\n--- RÉSULTATS DE L'ANALYSE DES COMPUTE UNITS ---");
    println!("-------------------------------------------------");
    println!("Copiez ces valeurs dans `src/execution/cu_manager.rs`\n");

    for r in results {
        match r.result {
            Ok(units) => println!("m.insert(\"{}\", DexCuCosts {{ base_cost: {}, cost_per_tick: None }});", r.name, units),
            Err(e) => println!("// ÉCHEC pour \"{}\": {}", r.name, e),
        }
    }

    println!("\nNOTE: Pour les CLMMs, la valeur ci-dessus est le coût de base (traversant 1-2 ticks).");
    println!("Pour obtenir `cost_per_tick`, lancez un test avec un montant 100x plus grand et faites la différence.");
    println!("-------------------------------------------------");

    Ok(())
}