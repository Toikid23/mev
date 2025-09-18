// DANS : src/communication.rs

use serde::{Deserialize, Serialize};
use solana_sdk::pubkey::Pubkey;
use yellowstone_grpc_proto::prelude as ysp;

// --- CONSTANTES CENTRALISÉES ---
pub const ZMQ_DATA_ENDPOINT: &str = "tcp://127.0.0.1:5555";
pub const ZMQ_COMMAND_ENDPOINT: &str = "tcp://127.0.0.1:5556";

// --- PROTOCOLE POUR LES COMMANDES ---
#[derive(Serialize, Deserialize, Debug)]
pub enum GatewayCommand {
    UpdateAccountSubscriptions(Vec<Pubkey>),
}

// --- PROTOCOLE POUR LES DONNÉES ---
#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Hash, Clone, Copy)]
pub enum ZmqTopic {
    Slot,
    Account,
    Transaction,
}

#[derive(Serialize, Deserialize, Debug)]
pub enum GeyserUpdate {
    Slot(SimpleSlotUpdate),
    Account(SimpleAccountUpdate),
    Transaction(SimpleTransactionUpdate),
}

// --- STRUCTURES MIROIRS SÉRIALISABLES ---

#[derive(Serialize, Deserialize, Debug)]
pub struct SimpleSlotUpdate {
    pub slot: u64,
    pub parent: Option<u64>,
}
impl From<ysp::SubscribeUpdateSlot> for SimpleSlotUpdate {
    fn from(value: ysp::SubscribeUpdateSlot) -> Self {
        Self { slot: value.slot, parent: value.parent }
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct SimpleAccountUpdate {
    pub pubkey: Pubkey,
    pub data: Vec<u8>,
    pub slot: u64,
}
impl From<ysp::SubscribeUpdateAccount> for SimpleAccountUpdate {
    fn from(value: ysp::SubscribeUpdateAccount) -> Self {
        let account_info = value.account.unwrap_or_default();
        Self {
            pubkey: Pubkey::try_from(account_info.pubkey).unwrap_or_default(),
            data: account_info.data,
            slot: value.slot,
        }
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct SerializableInstruction {
    pub program_id_index: u8,
    pub accounts: Vec<u8>,
    pub data: Vec<u8>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct SimpleTransactionUpdate {
    pub account_keys: Vec<Pubkey>,
    pub instructions: Vec<SerializableInstruction>,
    pub slot: u64,
}
impl From<ysp::SubscribeUpdateTransaction> for SimpleTransactionUpdate {
    fn from(value: ysp::SubscribeUpdateTransaction) -> Self {
        let (account_keys, instructions) = value.transaction.as_ref()
            .and_then(|tx_info| tx_info.transaction.as_ref())
            .and_then(|tx_meta| tx_meta.message.as_ref())
            .map(|message| {
                let keys = message.account_keys.iter()
                    .filter_map(|key_bytes| Pubkey::try_from(key_bytes.as_slice()).ok())
                    .collect();
                let ixs = message.instructions.iter()
                    .map(|ix| SerializableInstruction {
                        program_id_index: ix.program_id_index as u8,
                        accounts: ix.accounts.clone(),
                        data: ix.data.clone(),
                    })
                    .collect();
                (keys, ixs)
            })
            .unwrap_or_default();

        Self { account_keys, instructions, slot: value.slot }
    }
}