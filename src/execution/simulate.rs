// DANS : src/execution/simulate.rs (REMPLACEZ TOUT LE FICHIER)

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
use serde_json::{json, Value};

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

    // --- LA CORRECTION EST ICI ---
    // On transforme notre liste de comptes en un format JSON qui est un tableau de [pubkey_str, account_data_base64_str]
    let addresses_array: Vec<Value> = accounts_to_load
        .into_iter()
        .map(|(pubkey, account)| {
            let account_data_bincode = bincode::serialize(&account).unwrap();
            let account_data_base64 = base64::encode(account_data_bincode);
            json!([pubkey.to_string(), account_data_base64])
        })
        .collect();

    // On construit le paramètre `accounts` pour la simulation.
    // Le champ "addresses" attend maintenant un tableau (une "séquence").
    let accounts_param = json!({
        "encoding": "base64",
        "addresses": addresses_array
    });
    // --- FIN DE LA CORRECTION ---

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

    // La logique de parsing de la réponse ne change pas
    let rpc_response = response_value
        .get("value")
        .ok_or_else(|| anyhow!("Champ 'value' manquant dans la réponse de simulation"))?;

    let sim_result: RpcSimulateTransactionResult = serde_json::from_value(rpc_response.clone())?;

    if let Some(err) = &sim_result.err {
        if let Some(logs) = &sim_result.logs {
            println!("--- LOGS DE SIMULATION DÉTAILLÉS ---");
            for log in logs {
                println!("{}", log);
            }
            println!("------------------------------------");
        }
        return Err(anyhow!("La simulation de transaction a échoué : {:?}", err));
    }

    Ok(sim_result)
}