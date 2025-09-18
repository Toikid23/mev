use crate::config::Config;
use crate::rpc::ResilientRpcClient;
use anyhow::{anyhow, Context, Result};
// On revient à l'import qui fonctionne, même s'il est déprécié.
use solana_program::system_instruction;
use solana_sdk::{
    instruction::Instruction, message::VersionedMessage, pubkey::Pubkey, signature::Keypair,
    signer::Signer, transaction::VersionedTransaction,
};
use spl_associated_token_account::get_associated_token_address;
use spl_token::state::Account as SplTokenAccount;
use solana_program_pack::Pack;
use std::{sync::Arc, time::Duration};
use tokio::task::JoinHandle;
use tracing::{error, info, warn};


// On vérifie le solde toutes les 5 minutes.
const SOL_BALANCE_CHECK_INTERVAL_SECS: u64 = 300;

/// Lance une tâche de fond qui surveille le solde de SOL natif du portefeuille
/// et le recharge en déballant du WSOL si nécessaire.
pub fn start_balance_manager(
    rpc_client: Arc<ResilientRpcClient>,
    payer: Keypair,
    config: Config,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval =
            tokio::time::interval(Duration::from_secs(SOL_BALANCE_CHECK_INTERVAL_SECS));
        let wsol_mint = spl_token::native_mint::id();

        loop {
            interval.tick().await;

            match rpc_client.get_account(&payer.pubkey()).await {
                Ok(account) => {
                    info!("[BalanceManager] Solde SOL natif actuel : {} lamports", account.lamports);
                    if account.lamports < config.min_sol_balance {
                        warn!(
                            "[BalanceManager] Solde SOL bas ({}) sous le seuil de {}. Tentative d'unwrap de {} WSOL.",
                            account.lamports, config.min_sol_balance, config.unwrap_amount
                        );

                        if let Err(e) =
                            unwrap_wsol(&rpc_client, &payer, config.unwrap_amount, &wsol_mint)
                                .await
                        {
                            error!("[BalanceManager] Échec de l'unwrap de WSOL : {:?}", e);
                        }
                    }
                }
                Err(e) => {
                    error!("[BalanceManager] Impossible de récupérer le solde SOL du portefeuille : {}", e);
                }
            }
        }
    })
}

/// Construit et envoie une transaction atomique pour unwrap du WSOL de manière sûre
/// via un compte temporaire, sans jamais fermer l'ATA principal.
async fn unwrap_wsol(
    rpc_client: &Arc<ResilientRpcClient>,
    payer: &Keypair,
    amount: u64,
    wsol_mint: &Pubkey,
) -> Result<()> {
    let wsol_ata = get_associated_token_address(&payer.pubkey(), wsol_mint);

    // Étape 0 : Vérifier si le solde WSOL est suffisant
    match rpc_client.get_account(&wsol_ata).await {
        Ok(wsol_account) => {
            let wsol_balance = SplTokenAccount::unpack(&wsol_account.data)?.amount;
            if wsol_balance < amount {
                return Err(anyhow!(
                    "Solde WSOL insuffisant. Requis: {}, Disponible: {}",
                    amount, wsol_balance
                ));
            }
        }
        Err(_) => return Err(anyhow!("Le compte WSOL ATA n'existe pas ou est inaccessible.")),
    }

    // Étape 1 : Créer la keypair pour le compte temporaire en mémoire
    let temp_account_keypair = Keypair::new();
    // LA CORRECTION DE L'ERREUR EST ICI : on appelle notre nouvelle méthode sur le ResilientRpcClient
    let rent = rpc_client
        .get_minimum_balance_for_rent_exemption(SplTokenAccount::LEN)
        .await?;

    // Étape 2 : Construire les instructions
    let instructions: Vec<Instruction> = vec![
        // Instruction 1: Crée le compte qui va stocker temporairement le WSOL
        system_instruction::create_account(
            &payer.pubkey(),
            &temp_account_keypair.pubkey(),
            rent,
            SplTokenAccount::LEN as u64,
            &spl_token::id(),
        ),
        // Instruction 2: Initialise ce nouveau compte comme un compte de token WSOL
        spl_token::instruction::initialize_account(
            &spl_token::id(),
            &temp_account_keypair.pubkey(),
            wsol_mint,
            &payer.pubkey(), // Le propriétaire de ce compte temporaire est le payer
        )?,
        // Instruction 3: Transfère le WSOL de notre ATA principal vers le compte temporaire
        spl_token::instruction::transfer(
            &spl_token::id(),
            &wsol_ata,
            &temp_account_keypair.pubkey(),
            &payer.pubkey(), // Le payer est l'autorité de son propre ATA
            &[],
            amount,
        )?,
        // Instruction 4: Ferme le compte temporaire, ce qui libère le SOL natif
        spl_token::instruction::close_account(
            &spl_token::id(),
            &temp_account_keypair.pubkey(), // Le compte à fermer
            &payer.pubkey(),                // Les lamports (SOL natif) sont envoyés ici
            &payer.pubkey(),                // Le propriétaire du compte temporaire
            &[],
        )?,
    ];

    // Étape 3 : Créer et signer la transaction
    let recent_blockhash = rpc_client.get_latest_blockhash().await?;
    let message = VersionedMessage::V0(solana_sdk::message::v0::Message::try_compile(
        &payer.pubkey(),
        &instructions,
        &[],
        recent_blockhash,
    )?);

    // La transaction doit être signée par le payeur ET la clé temporaire qui autorise la fermeture.
    let transaction = VersionedTransaction::try_new(message, &[payer, &temp_account_keypair])?;

    // Étape 4 : Envoyer et confirmer
    let signature = rpc_client
        .send_and_confirm_transaction(&transaction)
        .await
        .context("La transaction d'unwrap a échoué à l'envoi ou à la confirmation")?;

    info!("[BalanceManager] Succès de l'unwrap de {} lamports de WSOL. Le solde SOL a été rechargé. Signature : {}", amount, signature);
    Ok(())
}