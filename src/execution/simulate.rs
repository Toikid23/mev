// DANS : src/execution/simulate.rs (Version FINALE et CORRECTE)

use anyhow::{anyhow, Result};
use solana_client::{
    nonblocking::rpc_client::RpcClient,
    rpc_config::RpcSimulateTransactionConfig,
    rpc_request::RpcRequest,
    rpc_response::{Response, RpcSimulateTransactionResult},
};
use solana_sdk::{
    account::Account,
    commitment_config::CommitmentConfig,
    instruction::Instruction,
    pubkey::Pubkey,
    signature::Keypair,
    signer::Signer,
    transaction::Transaction,
};
use solana_transaction_status::UiTransactionEncoding;
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

    let loaded_accounts: HashMap<String, String> = accounts_to_load
        .into_iter()
        .map(|(pubkey, account)| {
            let account_data = base64::encode(bincode::serialize(&account).unwrap_or_default());
            (pubkey.to_string(), account_data)
        })
        .collect();

    // Cette structure JSON est la seule qui est acceptée par le RPC
    let params = json!([
        encoded_tx,
        {
            "encoding": "base64",
            "sigVerify": false,
            "replaceRecentBlockhash": true,
            "commitment": "confirmed",
            "accounts": {
                "addresses": [], // Champ requis, même s'il est vide
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