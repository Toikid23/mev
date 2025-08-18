// DANS : src/bin/setup_pump_user.rs

use anyhow::Result;
use mev::{
    config::Config,
    decoders::pump_decoders::amm::{PUMP_PROGRAM_ID, DecodedPumpAmmPool, onchain_layouts}
};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::{
    pubkey::Pubkey,
    signer::{keypair::Keypair, Signer},
    transaction::Transaction,
    instruction::{Instruction, AccountMeta},
    system_program,
};
use std::str::FromStr;

// Cette fonction est une copie de celle dans pump/amm.rs pour créer l'instruction
fn create_init_user_volume_accumulator_instruction(user_owner: &Pubkey) -> Result<Instruction> {
    let discriminator: [u8; 8] = [94, 6, 202, 115, 255, 96, 232, 183];
    let (user_volume_accumulator, _) = Pubkey::find_program_address(&[b"user_volume_accumulator", user_owner.as_ref()], &PUMP_PROGRAM_ID);
    let (event_authority, _) = Pubkey::find_program_address(&[b"__event_authority"], &PUMP_PROGRAM_ID);

    let accounts = vec![
        AccountMeta::new(*user_owner, true),
        AccountMeta::new_readonly(*user_owner, false),
        AccountMeta::new(user_volume_accumulator, false),
        AccountMeta::new_readonly(system_program::id(), false),
        AccountMeta::new_readonly(event_authority, false),
        AccountMeta::new_readonly(PUMP_PROGRAM_ID, false),
    ];

    Ok(Instruction {
        program_id: PUMP_PROGRAM_ID,
        accounts,
        data: discriminator.to_vec(),
    })
}


#[tokio::main]
async fn main() -> Result<()> {
    println!("--- Outil d'Initialisation du Compte de Volume pump.fun ---");

    let config = Config::load()?;
    let rpc_client = RpcClient::new(config.solana_rpc_url);
    let payer = Keypair::from_base58_string(&config.payer_private_key);

    println!("Portefeuille Payeur: {}", payer.pubkey());

    let init_ix = create_init_user_volume_accumulator_instruction(&payer.pubkey())?;

    let recent_blockhash = rpc_client.get_latest_blockhash().await?;
    let mut transaction = Transaction::new_with_payer(&[init_ix], Some(&payer.pubkey()));
    transaction.sign(&[&payer], recent_blockhash);

    println!("\nEnvoi de la transaction pour créer le compte de volume utilisateur...");
    let signature = rpc_client.send_and_confirm_transaction(&transaction).await?;

    let (pda, _) = Pubkey::find_program_address(&[b"user_volume_accumulator", payer.pubkey().as_ref()], &PUMP_PROGRAM_ID);

    println!("\n✅ SUCCÈS !");
    println!("Le compte de volume a été créé (ou existe déjà).");
    println!("Adresse du PDA: {}", pda);
    println!("Signature de la transaction: {}", signature);
    println!("Votre portefeuille est maintenant prêt pour les simulations pump.fun.");

    Ok(())
}