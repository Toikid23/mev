
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

use anyhow::{Context, Result};
use futures_util::{sink::SinkExt, StreamExt};
use solana_sdk::pubkey::Pubkey;
use yellowstone_grpc_proto::geyser::SubscribeUpdateTransactionInfo;
use std::{collections::HashMap, time::Duration, str::FromStr};
use tokio::sync::Mutex;
use yellowstone_grpc_client::GeyserGrpcClient;
use yellowstone_grpc_proto::prelude::{
    subscribe_update::UpdateOneof, CommitmentLevel, SubscribeRequest,
    SubscribeRequestFilterTransactions,
};

use mev::{
    config::Config,
    decoders::{pump, raydium},
    filtering::{cache::PoolCache, PoolIdentity},
    // --- IMPORT DE NOTRE NOUVELLE MÉTRIQUE ---
    monitoring::metrics,
};

struct DexInfo {
    discriminators: Vec<[u8; 8]>,
    extractor: fn(&[Pubkey], &[u8]) -> Option<PoolIdentity>,
}

lazy_static::lazy_static! {
    static ref DEX_NAMES: HashMap<Pubkey, &'static str> = {
        let mut m = HashMap::new();
        m.insert(pump::amm::PUMP_PROGRAM_ID, "Pump.fun AMM");
        m.insert(Pubkey::from_str("CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C").unwrap(), "Raydium CPMM");
        m.insert(Pubkey::from_str("CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK").unwrap(), "Raydium CLMM");
        m.insert(Pubkey::from_str("675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8").unwrap(), "Raydium AMM V4");
        m.insert(Pubkey::from_str("whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc").unwrap(), "Orca Whirlpool");
        m.insert(Pubkey::from_str("LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo").unwrap(), "Meteora DLMM");
        m.insert(Pubkey::from_str("Eo7WjKq67rjJQSZxS6z3YkapzY3eMj6Xy8X5EQVn5UaB").unwrap(), "Meteora DAMM V1");
        m.insert(Pubkey::from_str("cpamdpZCGKUy5JxQXB4dcpGPiikHawvSWAd6mEn1sGG").unwrap(), "Meteora DAMM V2");
        m
    };

    static ref DEX_CREATE_INSTRUCTIONS: HashMap<Pubkey, DexInfo> = {
        let mut m = HashMap::new();

        // 1. Pump.fun (inchangé)
        m.insert(
            pump::amm::PUMP_PROGRAM_ID,
            DexInfo {
                discriminators: vec![
                    [233, 146, 209, 142, 207, 104, 64, 188], // create_pool
                ],
                extractor: extract_pump_identity,
            }
        );

        // 2. Raydium CPMM (inchangé)
        m.insert(
            Pubkey::from_str("CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C").unwrap(),
            DexInfo {
                discriminators: vec![
                    [175, 175, 109, 31, 13, 152, 155, 237], // initialize
                    [63, 55, 254, 65, 49, 178, 89, 121],    // initialize_with_permission
                ],
                extractor: extract_raydium_cpmm_identity,
            }
        );

        // --- NOUVELLE ENTRÉE POUR RAYDIUM CLMM ---
        m.insert(
            Pubkey::from_str("CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK").unwrap(),
            DexInfo {
                discriminators: vec![
                    [233, 146, 209, 142, 207, 104, 64, 188], // create_pool
                ],
                extractor: extract_raydium_clmm_identity,
            }
        );
// --- NOUVELLE ENTRÉE POUR ORCA WHIRLPOOL ---
        m.insert(
            Pubkey::from_str("whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc").unwrap(),
            DexInfo {
                discriminators: vec![
                    [17, 10, 13, 10, 215, 16, 243, 220], // initialize_pool
                ],
                extractor: extract_orca_whirlpool_identity,
            }
        );

        m.insert(
            Pubkey::from_str("LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo").unwrap(),
            DexInfo {
                discriminators: vec![
                    [18, 18, 129, 17, 45, 184, 15, 203],    // initialize_lb_pair
                    [116, 219, 107, 218, 12, 10, 31, 102], // initialize_customizable_permissionless_lb_pair
                ],
                extractor: extract_meteora_dlmm_identity,
            }
        );

        m.insert(
            Pubkey::from_str("Eo7WjKq67rjJQSZxS6z3YkapzY3eMj6Xy8X5EQVn5UaB").unwrap(),
            DexInfo {
                discriminators: vec![
                    [1, 9, 22, 238, 15, 115, 22, 18], // initialize_permissionless_pool
                ],
                extractor: extract_meteora_damm_v1_identity,
            }
        );

        // --- NOUVELLE ENTRÉE POUR METEORA DAMM V2 ---
        m.insert(
            Pubkey::from_str("cpamdpZCGKUy5JxQXB4dcpGPiikHawvSWAd6mEn1sGG").unwrap(),
            DexInfo {
                discriminators: vec![
                    [95, 180, 10, 172, 84, 174, 232, 40], // initialize_pool
                ],
                extractor: extract_meteora_damm_v2_identity,
            }
        );

        m
    };
    static ref UNIVERSE_FILE_LOCK: Mutex<()> = Mutex::new(());
}


// --- NOUVELLE FONCTION POUR LE LOGGING PÉRIODIQUE ---
async fn log_stats_periodically(stats_map: std::sync::Arc<Mutex<HashMap<Pubkey, u64>>>) {
    let mut interval = tokio::time::interval(Duration::from_secs(300)); // Toutes les 5 minutes
    loop {
        interval.tick().await;
        let mut stats = stats_map.lock().await;
        if !stats.is_empty() {
            println!("\n--- [Stats Listener] Nouvelles Pools Découvertes (dernières 5 minutes) ---");
            for (program_id, count) in stats.iter() {
                let dex_name = DEX_NAMES.get(program_id).unwrap_or(&"DEX Inconnu");
                println!("  -> {}: {} pool(s)", dex_name, count);
            }
            println!("--------------------------------------------------------------------");
            stats.clear(); // On réinitialise pour la prochaine période
        }
    }
}


#[tokio::main]
async fn main() -> Result<()> {
    println!("--- Démarrage du Pool Listener (Multi-DEX) ---");
    let _config = Config::load()?;
    let geyser_url = "http://127.0.0.1:10000";

    // On lance le serveur de métriques pour que Prometheus puisse nous scraper
    tokio::spawn(metrics::start_metrics_server());
    println!("[Listener] Serveur de métriques Prometheus démarré sur le port 9100.");

    // On crée l'état partagé ici
    let stats_map = std::sync::Arc::new(Mutex::new(HashMap::new()));

    // On lance la tâche de logging en arrière-plan
    tokio::spawn(log_stats_periodically(stats_map.clone()));

    loop {
        println!("[Listener] Tentative de connexion à Geyser : {}", geyser_url);
        match run_listener(geyser_url).await {
            Ok(_) => eprintln!("[Listener] Le stream Geyser s'est terminé. Reconnexion..."),
            Err(e) => eprintln!("[Listener] Erreur dans le stream Geyser : {}. Reconnexion dans 5s...", e),
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

async fn run_listener(geyser_url: &str) -> Result<()> {
    let mut client = GeyserGrpcClient::build_from_shared(geyser_url.to_string())?
        .connect().await.context("Connexion au client Geyser gRPC échouée")?;

    let programs_to_watch: Vec<String> = DEX_CREATE_INSTRUCTIONS.keys().map(|k| k.to_string()).collect();
    println!("[Listener] Surveillance des programmes : {:?}", programs_to_watch);

    let mut tx_filter = HashMap::new();
    tx_filter.insert(
        "txs".to_string(),
        SubscribeRequestFilterTransactions {
            vote: Some(false), failed: Some(false), account_include: vec![],
            account_required: programs_to_watch,
            account_exclude: vec![], signature: None,
        },
    );
    let request = SubscribeRequest {
        transactions: tx_filter,
        commitment: Some(CommitmentLevel::Confirmed as i32),
        ..Default::default()
    };
    let (mut tx, mut stream) = client.subscribe().await?;
    tx.send(request).await?;
    println!("[Listener] Abonnement réussi. En attente des transactions de création...");

    while let Some(message_result) = stream.next().await {
        let message = message_result.context("Erreur dans le stream Geyser")?;

        // --- CORRECTION DE LA FAUTE DE FRAPPE ICI ---
        if let Some(UpdateOneof::Transaction(tx_update)) = message.update_oneof {
            if let Some(tx_info) = tx_update.transaction {
                process_transaction(tx_info).await;
            }
        }
    }
    Ok(())
}

async fn process_transaction(tx_info: SubscribeUpdateTransactionInfo) {
    // ... (la logique de décodage est inchangée)
    let Some(tx_meta) = tx_info.transaction else { return };
    let Some(message) = tx_meta.message else { return };
    let account_keys: Vec<Pubkey> = message.account_keys.iter().filter_map(|key_bytes| Pubkey::try_from(key_bytes.as_slice()).ok()).collect();
    if account_keys.len() != message.account_keys.len() { return; }

    for ix in &message.instructions {
        let Some(program_id) = account_keys.get(ix.program_id_index as usize) else { continue };
        let mut identity_found: Option<PoolIdentity> = None;
        if *program_id == raydium::amm_v4::RAYDIUM_AMM_V4_PROGRAM_ID {
            if ix.data.starts_with(&[0]) {
                identity_found = extract_raydium_amm_v4_identity(&account_keys, &ix.accounts, 0);
            } else if ix.data.starts_with(&[1]) {
                identity_found = extract_raydium_amm_v4_identity(&account_keys, &ix.accounts, 1);
            }
        } else if let Some(dex_info) = DEX_CREATE_INSTRUCTIONS.get(program_id) {
            for discriminator in &dex_info.discriminators {
                if ix.data.starts_with(discriminator) {
                    identity_found = (dex_info.extractor)(&account_keys, &ix.accounts);
                    if identity_found.is_some() { break; }
                }
            }
        }

        if let Some(new_identity) = identity_found {
            // --- MODIFIÉ : On utilise la métrique Prometheus ---
            let dex_name = DEX_NAMES.get(program_id).unwrap_or(&"Unknown DEX");
            metrics::NEW_POOLS_DISCOVERED.with_label_values(&[dex_name]).inc();

            println!("[Listener] ✨ Nouvelle pool [{}] détectée : {}", dex_name, new_identity.address);

            if let Err(e) = add_pool_to_universe(new_identity).await {
                eprintln!("[Listener] Erreur de mise à jour du cache : {}", e);
            }
            break;
        }
    }
}

// --- Extracteur spécifique pour Pump.fun ---
fn extract_pump_identity(account_keys: &[Pubkey], ix_accounts: &[u8]) -> Option<PoolIdentity> {
    let pool_key = *ix_accounts.get(0).and_then(|&idx| account_keys.get(idx as usize))?;
    let base_mint_key = *ix_accounts.get(3).and_then(|&idx| account_keys.get(idx as usize))?;
    let quote_mint_key = *ix_accounts.get(4).and_then(|&idx| account_keys.get(idx as usize))?;

    let (vault_a, _) = Pubkey::find_program_address(&[b"pool_base_vault", pool_key.as_ref()], &pump::amm::PUMP_PROGRAM_ID);
    let (vault_b, _) = Pubkey::find_program_address(&[b"pool_quote_vault", pool_key.as_ref()], &pump::amm::PUMP_PROGRAM_ID);

    Some(PoolIdentity {
        address: pool_key,
        mint_a: base_mint_key,
        mint_b: quote_mint_key,
        accounts_to_watch: vec![vault_a, vault_b],
    })
}

// --- Extracteur spécifique pour Raydium CPMM ---
fn extract_raydium_cpmm_identity(account_keys: &[Pubkey], ix_accounts: &[u8]) -> Option<PoolIdentity> {
    // Les comptes clés sont aux mêmes indices pour `initialize` et `initialize_with_permission`
    let pool_key = *ix_accounts.get(4).and_then(|&idx| account_keys.get(idx as usize))?;
    let mint_a_key = *ix_accounts.get(5).and_then(|&idx| account_keys.get(idx as usize))?;
    let mint_b_key = *ix_accounts.get(6).and_then(|&idx| account_keys.get(idx as usize))?;

    // Pour un CPMM, les comptes à surveiller sont les vaults, qui sont des PDAs.
    // Le programme Raydium CPMM n'exporte pas ses seeds, nous devons donc les reconstruire
    // en nous basant sur le code source ou la documentation.
    // Heureusement, votre décodeur les contient déjà !
    let (vault_a, _) = Pubkey::find_program_address(
        &[b"pool_vault", pool_key.as_ref(), mint_a_key.as_ref()],
        &Pubkey::from_str("CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C").unwrap(),
    );
    let (vault_b, _) = Pubkey::find_program_address(
        &[b"pool_vault", pool_key.as_ref(), mint_b_key.as_ref()],
        &Pubkey::from_str("CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C").unwrap(),
    );

    Some(PoolIdentity {
        address: pool_key,
        mint_a: mint_a_key,
        mint_b: mint_b_key,
        accounts_to_watch: vec![vault_a, vault_b],
    })
}

fn extract_raydium_clmm_identity(account_keys: &[Pubkey], ix_accounts: &[u8]) -> Option<PoolIdentity> {
    let pool_key = *ix_accounts.get(2).and_then(|&idx| account_keys.get(idx as usize))?;
    let mint_a_key = *ix_accounts.get(3).and_then(|&idx| account_keys.get(idx as usize))?;
    let mint_b_key = *ix_accounts.get(4).and_then(|&idx| account_keys.get(idx as usize))?;

    // Pour les CLMM, la logique de swap dépend principalement des changements dans le compte
    // de pool lui-même (sqrt_price, liquidity). C'est donc ce compte que nous devons surveiller.
    Some(PoolIdentity {
        address: pool_key,
        mint_a: mint_a_key,
        mint_b: mint_b_key,
        accounts_to_watch: vec![pool_key], // <-- POINT IMPORTANT
    })
}

fn extract_raydium_amm_v4_identity(account_keys: &[Pubkey], ix_accounts: &[u8], discriminator: u8) -> Option<PoolIdentity> {
    // L'ordre des comptes varie légèrement entre `initialize` (disc 0) et `initialize2` (disc 1)
    let (pool_idx, mint_a_idx, mint_b_idx) = if discriminator == 0 {
        (3, 7, 8)
    } else { // discriminator == 1
        (4, 8, 9)
    };

    let pool_key = *ix_accounts.get(pool_idx).and_then(|&idx| account_keys.get(idx as usize))?;
    let mint_a_key = *ix_accounts.get(mint_a_idx).and_then(|&idx| account_keys.get(idx as usize))?;
    let mint_b_key = *ix_accounts.get(mint_b_idx).and_then(|&idx| account_keys.get(idx as usize))?;

    // Pour les AMM, on surveille les vaults. Votre décodeur AMM V4 hydrate ces vaults
    // à partir du compte du marché Serum, mais pour le listener, nous avons besoin d'une
    // méthode déterministe pour trouver leurs adresses sans un appel RPC.
    // Heureusement, ce sont des PDAs dérivés du programme AMM lui-même.
    let (vault_a, _) = Pubkey::find_program_address(
        &[b"amm_associated_seed", pool_key.as_ref(), mint_a_key.as_ref()],
        &raydium::amm_v4::RAYDIUM_AMM_V4_PROGRAM_ID,
    );
    let (vault_b, _) = Pubkey::find_program_address(
        &[b"amm_associated_seed", pool_key.as_ref(), mint_b_key.as_ref()],
        &raydium::amm_v4::RAYDIUM_AMM_V4_PROGRAM_ID,
    );

    Some(PoolIdentity {
        address: pool_key,
        mint_a: mint_a_key,
        mint_b: mint_b_key,
        accounts_to_watch: vec![vault_a, vault_b],
    })
}

fn extract_orca_whirlpool_identity(account_keys: &[Pubkey], ix_accounts: &[u8]) -> Option<PoolIdentity> {
    let mint_a_key = *ix_accounts.get(1).and_then(|&idx| account_keys.get(idx as usize))?;
    let mint_b_key = *ix_accounts.get(2).and_then(|&idx| account_keys.get(idx as usize))?;
    let pool_key = *ix_accounts.get(4).and_then(|&idx| account_keys.get(idx as usize))?;

    // Pour les Whirlpools (CLMM), l'activité principale (changement de prix) se produit
    // sur le compte du pool lui-même. C'est donc ce compte que nous surveillons.
    Some(PoolIdentity {
        address: pool_key,
        mint_a: mint_a_key,
        mint_b: mint_b_key,
        accounts_to_watch: vec![pool_key], // <-- On surveille l'adresse du pool
    })
}

fn extract_meteora_dlmm_identity(account_keys: &[Pubkey], ix_accounts: &[u8]) -> Option<PoolIdentity> {
    let pool_key = *ix_accounts.get(0).and_then(|&idx| account_keys.get(idx as usize))?;
    let mint_a_key = *ix_accounts.get(2).and_then(|&idx| account_keys.get(idx as usize))?;
    let mint_b_key = *ix_accounts.get(3).and_then(|&idx| account_keys.get(idx as usize))?;

    // Pour le DLMM, comme les autres CLMM, l'activité se reflète sur le compte du pool lui-même.
    Some(PoolIdentity {
        address: pool_key,
        mint_a: mint_a_key,
        mint_b: mint_b_key,
        accounts_to_watch: vec![pool_key],
    })
}

fn extract_meteora_damm_v1_identity(account_keys: &[Pubkey], ix_accounts: &[u8]) -> Option<PoolIdentity> {
    let pool_key = *ix_accounts.get(0).and_then(|&idx| account_keys.get(idx as usize))?;
    let mint_a_key = *ix_accounts.get(2).and_then(|&idx| account_keys.get(idx as usize))?;
    let mint_b_key = *ix_accounts.get(3).and_then(|&idx| account_keys.get(idx as usize))?;

    // Les pools DAMM dépendent de vaults externes pour leur liquidité, mais
    // ces vaults ne sont pas créés dans la même instruction. Cependant, les swaps
    // sur ces pools impliquent toujours des changements sur les comptes de vault.
    // Votre décodeur DAMM V1 hydrate déjà ces vaults, nous pouvons donc les extraire.
    let (a_vault, _) = Pubkey::find_program_address(&[b"vault", pool_key.as_ref(), mint_a_key.as_ref()], &Pubkey::from_str("24Uqj9JCLxUeoC3hGfh5W3s9FM9uCHDS2SG3LYwBpyTi").unwrap());
    let (b_vault, _) = Pubkey::find_program_address(&[b"vault", pool_key.as_ref(), mint_b_key.as_ref()], &Pubkey::from_str("24Uqj9JCLxUeoC3hGfh5W3s9FM9uCHDS2SG3LYwBpyTi").unwrap());

    Some(PoolIdentity {
        address: pool_key,
        mint_a: mint_a_key,
        mint_b: mint_b_key,
        accounts_to_watch: vec![a_vault, b_vault],
    })
}

// --- Extracteur spécifique pour Meteora DAMM V2 ---
fn extract_meteora_damm_v2_identity(account_keys: &[Pubkey], ix_accounts: &[u8]) -> Option<PoolIdentity> {
    let pool_key = *ix_accounts.get(6).and_then(|&idx| account_keys.get(idx as usize))?;
    let mint_a_key = *ix_accounts.get(9).and_then(|&idx| account_keys.get(idx as usize))?;
    let mint_b_key = *ix_accounts.get(10).and_then(|&idx| account_keys.get(idx as usize))?;

    // DAMM V2 est de type CLMM, on surveille donc le pool state directement.
    Some(PoolIdentity {
        address: pool_key,
        mint_a: mint_a_key,
        mint_b: mint_b_key,
        accounts_to_watch: vec![pool_key],
    })
}

// --- La fonction de sauvegarde reste la même ---
async fn add_pool_to_universe(new_identity: PoolIdentity) -> Result<()> {
    // ... (code inchangé)
    let _lock = UNIVERSE_FILE_LOCK.lock().await;
    let cache = PoolCache::load().unwrap_or_else(|_| PoolCache {
        pools: HashMap::new(),
        watch_map: HashMap::new(),
    });
    let mut identities: Vec<PoolIdentity> = cache.pools.into_values().collect();
    if identities.iter().any(|id| id.address == new_identity.address) {
        println!("[Listener] Le pool {} est déjà dans l'univers. Ignoré.", new_identity.address);
        return Ok(());
    }
    identities.push(new_identity);
    PoolCache::save(&identities)?;
    println!("[Listener] ✅ Le pool a été ajouté avec succès à pools_universe.json !");
    Ok(())
}