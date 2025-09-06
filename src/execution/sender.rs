use anyhow::{Result};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::message::AddressLookupTableAccount;
use solana_sdk::{
    compute_budget::ComputeBudgetInstruction,
    instruction::{Instruction},
    message::{v0, VersionedMessage},
    signature::{Keypair, Signer},
    transaction::VersionedTransaction,
};
use std::sync::Arc;

/// Reconstruit et envoie une transaction normale en y ajoutant les instructions de Compute Budget.
pub async fn send_normal_transaction(
    rpc_client: Arc<RpcClient>,
    payer: &Keypair,
    execute_route_ix: Instruction,
    lookup_table: &AddressLookupTableAccount,
    compute_units: u64,
    priority_fee_price_per_cu: u64,
) -> Result<solana_sdk::signature::Signature> {
    println!("  -> Préparation de la transaction normale pour envoi...");

    // Étape 1: Créer les instructions de Compute Budget.
    let set_compute_unit_limit_ix = ComputeBudgetInstruction::set_compute_unit_limit(compute_units as u32);
    let set_compute_unit_price_ix = ComputeBudgetInstruction::set_compute_unit_price(priority_fee_price_per_cu);

    // Étape 2: Assembler la liste finale d'instructions.
    let final_instructions = vec![
        set_compute_unit_limit_ix,
        set_compute_unit_price_ix,
        execute_route_ix,
    ];

    let recent_blockhash = rpc_client.get_latest_blockhash().await?;

    // Étape 3: Compiler le message.
    let final_message = v0::Message::try_compile(
        &payer.pubkey(),
        &final_instructions,
        &[lookup_table.clone()],
        recent_blockhash,
    )?;

    // Étape 4: Créer et signer la transaction.
    let tx_to_send = VersionedTransaction::try_new(VersionedMessage::V0(final_message), &[payer])?;

    // Étape 5: Envoyer la transaction.
    println!("  -> Envoi de la transaction et attente de la confirmation...");
    let signature = rpc_client.send_and_confirm_transaction(&tx_to_send).await?;

    Ok(signature)
}
/// Placeholder pour la logique d'envoi de bundle Jito.
pub async fn send_jito_bundle(
    _final_arbitrage_tx: &VersionedTransaction,
    _jito_tip: u64,
) -> Result<()> {
    // TODO: Intégrer le client Jito ici.
    // 1. Créer une transaction de tip Jito.
    // 2. Créer un bundle avec `final_arbitrage_tx` et la transaction de tip.
    // 3. Envoyer le bundle via le client Jito.
    println!("  -> [JITO] La logique d'envoi de bundle sera implémentée ici.");
    Ok(())
}