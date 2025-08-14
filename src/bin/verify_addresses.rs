// DANS : src/bin/verify_addresses.rs

use anyhow::{Result, bail};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;
use std::collections::HashMap;

use mev::{
    config::Config,
    decoders::raydium_decoders::amm_v4,
};

// Fonction pour vérifier un ensemble de comptes et afficher leur statut
async fn check_accounts(
    rpc_client: &RpcClient,
    accounts_to_check: HashMap<&str, Pubkey>
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
    println!("--- Lancement du Vérificateur d'Adresses Hydratées ---");
    let config = Config::load()?;
    let rpc_client = RpcClient::new(config.solana_rpc_url);

    const POOL_ADDRESS: &str = "58oQChx4yWmvKdwLLZzBi4ChoCc2fqCUWBkwMihLYQo2"; // WSOL-USDC
    println!("\n[1/2] Décodage et hydratation du pool : {}", POOL_ADDRESS);

    let pool_pubkey = Pubkey::from_str(POOL_ADDRESS)?;

    let account_data = match rpc_client.get_account_data(&pool_pubkey).await {
        Ok(data) => data,
        Err(e) => {
            bail!("Impossible de récupérer les données du compte de pool {}. Assurez-vous que l'URL RPC est correcte et pointe vers le mainnet. Erreur : {}", POOL_ADDRESS, e);
        }
    };

    if account_data.is_empty() {
        bail!("Les données du compte de pool {} sont vides. Le compte n'existe probablement pas sur ce cluster RPC.", POOL_ADDRESS);
    }

    let mut pool = match amm_v4::decode_pool(&pool_pubkey, &account_data) {
        Ok(p) => p,
        Err(e) => {
            bail!("Le décodage initial du pool a échoué : {}", e);
        }
    };

    if let Err(e) = amm_v4::hydrate(&mut pool, &rpc_client).await {
        bail!("L'hydratation a échoué. Cela peut être dû à une erreur RPC ou à un problème de parsing des comptes dépendants (ex: marché). Erreur : {}", e);
    };

    println!("-> Hydratation terminée avec succès.");

    println!("\n[2/2] Vérification de l'existence de tous les comptes extraits sur la blockchain...");

    let mut accounts = HashMap::new();
    accounts.insert("Pool AMM", pool.address);
    accounts.insert("Vault A (Coin)", pool.vault_a);
    accounts.insert("Vault B (PC)", pool.vault_b);
    accounts.insert("Mint A", pool.mint_a);
    accounts.insert("Mint B", pool.mint_b);
    accounts.insert("Marché", pool.market);
    accounts.insert("Open Orders", pool.open_orders);
    accounts.insert("Target Orders", pool.target_orders);
    accounts.insert("Market Bids", pool.market_bids);
    accounts.insert("Market Asks", pool.market_asks);
    accounts.insert("Market Event Queue", pool.market_event_queue);
    accounts.insert("Market Coin Vault", pool.market_coin_vault);
    accounts.insert("Market PC Vault", pool.market_pc_vault);
    accounts.insert("Market Vault Signer", pool.market_vault_signer);

    let all_accounts_found = check_accounts(&rpc_client, accounts).await?;

    println!("\n--- CONCLUSION ---");
    if all_accounts_found {
        println!("✅ SUCCÈS ! Toutes les adresses extraites par la fonction `hydrate` correspondent à des comptes réels sur la blockchain.");
        println!("Le problème de 'AccountNotFound' vient probablement du CONTEXTE de la simulation, pas du parsing.");

    } else {
        println!("❌ ÉCHEC ! Au moins une des adresses extraites par la fonction `hydrate` est incorrecte ou n'existe pas.");
        println!("Ceci est la cause la plus probable de l'erreur 'AccountNotFound'. Veuillez corriger les offsets ou la logique de dérivation dans `amm_v4.rs`.");
    }

    Ok(())
}