// DANS : src/execution/simulate.rs (Version NETTOYÉE)

use anyhow::{anyhow, Result};
// CORRECTION: Imports inutiles retirés
use solana_client::{
    nonblocking::rpc_client::RpcClient,
    rpc_request::RpcRequest,
    rpc_response::{Response, RpcSimulateTransactionResult},
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
// CORRECTION: Imports nécessaires pour la nouvelle API base64
use base64::{engine::general_purpose, Engine as _};


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
    // CORRECTION: Utilisation de la nouvelle API base64
    let encoded_tx = general_purpose::STANDARD.encode(serialized_tx);

    let loaded_accounts: HashMap<String, String> = accounts_to_load
        .into_iter()
        .map(|(pubkey, account)| {
            let account_data = general_purpose::STANDARD.encode(bincode::serialize(&account).unwrap_or_default());
            (pubkey.to_string(), account_data)
        })
        .collect();

    let params = json!([
        encoded_tx,
        {
            "encoding": "base64",
            "sigVerify": false,
            "replaceRecentBlockhash": true,
            "commitment": "confirmed",
            "accounts": {
                "addresses": [],
                "loaded": loaded_accounts
            }
        }
    ]);

    let request = RpcRequest::Custom { method: "simulateTransaction" };
    let response_value: serde_json::Value = rpc_client.send(request, params).await?;
    let response: Response<RpcSimulateTransactionResult> = serde_json::from_value(response_value)?;

    if let Some(err) = response.value.err {
        return Err(anyhow!("La simulation de transaction a échoué : {:?}", err));
    }
    Ok(response.value)
}