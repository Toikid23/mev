// DANS : src/execution/simulate.rs

use anyhow::{anyhow, Result};
use solana_client::{
    nonblocking::rpc_client::RpcClient,
    rpc_response::{Response, RpcSimulateTransactionResult},
    rpc_request::RpcRequest,
};
use solana_sdk::{
    account::Account,
    instruction::Instruction,
    pubkey::Pubkey,
    signature::Keypair,
    signer::Signer,
    transaction::Transaction,
};
use std::collections::HashMap;
use serde_json::json;

pub async fn simulate_instruction(
    rpc_client: &RpcClient,
    instruction: Instruction,
    payer: &Keypair,
    accounts_to_load: Vec<(Pubkey, Account)>,
) -> Result<RpcSimulateTransactionResult> {

    let mut transaction = Transaction::new_with_payer(&[instruction], Some(&payer.pubkey()));
    let recent_blockhash = rpc_client.get_latest_blockhash().await?;
    transaction.sign(&[payer], recent_blockhash);

    let serialized_tx = bincode::serialize(&transaction)?;
    let encoded_tx = base64::encode(serialized_tx);

    let loaded_accounts: Vec<(String, String)> = accounts_to_load // <-- Doit être un Vec de tuples
        .into_iter()
        .map(|(pubkey, account)| {
            let account_data_bincode = bincode::serialize(&account).unwrap_or_default();
            let account_data_base64 = base64::encode(account_data_bincode);
            (pubkey.to_string(), account_data_base64)
        })
        .collect();

    // --- LA CORRECTION EST ICI ---
    // Le champ "addresses" ne doit pas être une Map, mais une Séquence (un Array) de strings.
    // L'API de simulation avec `loaded` est cachée et non standard, et très capricieuse.
    // Nous revenons à la méthode `replaceRecentBlockhash` qui est plus stable.

    let params = json!([
        encoded_tx,
        {
            "encoding": "base64",
            "sigVerify": false,
            "replaceRecentBlockhash": true, // <-- On utilise cette méthode plus fiable
            "commitment": "confirmed"
        }
    ]);

    let request = RpcRequest::Custom { method: "simulateTransaction" };
    let response_value: serde_json::Value = rpc_client.send(request, params).await?;

    // Le RPC Helius retourne une structure légèrement différente. Nous devons la parser manuellement.
    let rpc_response = response_value
        .get("value")
        .ok_or_else(|| anyhow!("Champ 'value' manquant dans la réponse de simulation"))?;

    let sim_result: RpcSimulateTransactionResult = serde_json::from_value(rpc_response.clone())?;

    if let Some(err) = &sim_result.err {
        return Err(anyhow!("La simulation de transaction a échoué : {:?}", err));
    }

    Ok(sim_result)
}