// DANS : src/execution/simulate.rs (VERSION FINALE - Correcte pour les RPCs avancés)

use anyhow::{anyhow, Result};
use solana_client::{
    nonblocking::rpc_client::RpcClient,
    rpc_response::{RpcSimulateTransactionResult},
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
use serde_json::{json, Map, Value};

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

    // Le format le plus commun pour les RPCs supportant cette feature est une Map (un objet JSON).
    let mut address_map = Map::new();
    for (pubkey, account) in accounts_to_load {
        let account_data_bincode = bincode::serialize(&account)?;
        let account_data_base64 = base64::encode(account_data_bincode);
        address_map.insert(pubkey.to_string(), Value::String(account_data_base64));
    }

    let accounts_param = json!({
        "encoding": "base64",
        "addresses": address_map // On utilise la Map
    });

    let params = json!([
        encoded_tx,
        {
            "encoding": "base64",
            "sigVerify": false,
            "replaceRecentBlockhash": true,
            "commitment": "confirmed",
            "accounts": accounts_param
        }
    ]);

    let request = RpcRequest::Custom { method: "simulateTransaction" };
    let response_value: serde_json::Value = rpc_client.send(request, params).await?;

    let rpc_response = response_value
        .get("value")
        .ok_or_else(|| anyhow!("Champ 'value' manquant dans la réponse de simulation"))?;

    let sim_result: RpcSimulateTransactionResult = serde_json::from_value(rpc_response.clone())?;

    if let Some(err) = &sim_result.err {
        return Err(anyhow!("La simulation de transaction a échoué : {:?}", err));
    }

    Ok(sim_result)
}