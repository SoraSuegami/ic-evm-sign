#[cfg(not(test))]
use ic_cdk::api::time as ic_timestamp;
#[cfg(not(test))]
use ic_cdk::api::call::call_with_payment as ic_call;
use ic_cdk::export::{
    candid::CandidType,
    serde::{Deserialize, Serialize},
    Principal,
};
#[cfg(test)]
mod mocks;
#[cfg(test)]
use mocks::{ic_call, ic_timestamp};

mod utils;
pub use utils::u64_to_u256;
use utils::{get_address_from_public_key, get_derivation_path};

mod ecdsa;
use ecdsa::reply::*;
use ecdsa::request::*;

pub mod state;
use state::*;

pub mod transaction;
use transaction::*;

#[derive(CandidType, Serialize, Debug)]
pub struct CreateAddressResponse {
    pub address: String,
}
#[derive(CandidType, Deserialize, Debug)]
pub struct SignTransactionResponse {
    pub sign_tx: Vec<u8>,
}
#[derive(CandidType, Deserialize, Debug)]
pub struct DeployContractResponse {
    pub tx: Vec<u8>,
}
#[derive(CandidType, Deserialize, Debug)]
pub struct TransferERC20Response {
    pub tx: Vec<u8>,
}
#[derive(CandidType, Deserialize, Debug)]
pub struct UserResponse {
    pub address: String,
    pub transactions: TransactionChainData,
}

pub fn init(env_opt: Option<Environment>) {
    if let Some(env) = env_opt {
        STATE.with(|s| {
            let mut state = s.borrow_mut();
            state.config = Config::from(env);
        })
    }
}

pub async fn create_address(principal_id: Principal) -> Result<CreateAddressResponse, String> {
    let state = STATE.with(|s| s.borrow().clone());
    let user = state.users.get(&principal_id);

    if let Some(_) = user {
        return Err("this wallet already exist".to_string());
    }

    let key_id = EcdsaKeyId {
        curve: EcdsaCurve::Secp256k1,
        name: state.config.key_name
    };

    let caller = get_derivation_path(principal_id);

    let request = ECDSAPublicKey {
        canister_id: None,
        derivation_path: vec![caller],
        key_id: key_id.clone(),
    };

    let (res,): (ECDSAPublicKeyResponse,) = ic_call(
        Principal::management_canister(),
        "ecdsa_public_key",
        (request,),
        0 as u64
    )
    .await
    .map_err(|e| format!("Failed to call ecdsa_public_key {}", e.1))?;

    let address = get_address_from_public_key(res.public_key.clone()).unwrap();

    let mut user = UserData::default();
    user.public_key = res.public_key;

    STATE.with(|s| {
        let mut state = s.borrow_mut();
        state.users.insert(principal_id, user);
    });

    Ok(CreateAddressResponse { address })
}

pub async fn sign_transaction(
    hex_raw_tx: Vec<u8>,
    chain_id: u64,
    principal_id: Principal,
) -> Result<SignTransactionResponse, String> {
    let state = STATE.with(|s| s.borrow().clone());
    let user;

    if let Some(i) = state.users.get(&principal_id) {
        user = i.clone();
    } else {
        return Err("this user does not exist".to_string());
    }

    let mut tx = transaction::get_transaction(&hex_raw_tx, chain_id.clone()).unwrap();

    let message = tx.get_message_to_sign().unwrap();

    assert!(message.len() == 32);

    let key_id = EcdsaKeyId {
        curve: EcdsaCurve::Secp256k1,
        name: state.config.key_name,
    };

    let caller = get_derivation_path(principal_id);

    let request = SignWithECDSA {
        message_hash: message.clone(),
        derivation_path: vec![caller],
        key_id: key_id.clone(),
    };

    let (res,): (SignWithECDSAResponse,) = ic_call(
        Principal::management_canister(),
        "sign_with_ecdsa",
        (request,),
        state.config.sign_cycles
    )
    .await
    .map_err(|e| format!("Failed to call sign_with_ecdsa {}", e.1))?;

    let signed_tx = tx.sign(res.signature.clone(), user.public_key).unwrap();

    STATE.with(|s| {
        let mut state = s.borrow_mut();
        let user = state.users.get_mut(&principal_id).unwrap();

        let mut transaction = Transaction::default();
        transaction.data = signed_tx.clone();
        transaction.timestamp = ic_timestamp();

        if let Some(user_tx) = user.transactions.get_mut(&chain_id) {
            user_tx.transactions.push(transaction);
            user_tx.nonce = utils::u256_to_u64(tx.get_nonce().unwrap()) + 1;
        } else {
            let mut chain_data = TransactionChainData::default();
            chain_data.nonce = utils::u256_to_u64(tx.get_nonce().unwrap()) + 1;
            chain_data.transactions.push(transaction);

            user.transactions.insert(chain_id, chain_data);
        }
    });

    Ok(SignTransactionResponse { sign_tx: signed_tx })
}

pub async fn deploy_contract(
    principal_id: Principal,
    bytecode: Vec<u8>,
    chain_id: u64,
    max_priority_fee_per_gas: U256,
    gas_limit: U256,
    max_fee_per_gas: U256,
) -> Result<DeployContractResponse, String> {
    let users = STATE.with(|s| s.borrow().users.clone());
    let user;

    if let Some(i) = users.get(&principal_id) {
        user = i.clone();
    } else {
        return Err("this user does not exist".to_string());
    }

    let nonce: U256;
    if let Some(user_transactions) = user.transactions.get(&chain_id) {
        nonce = utils::u64_to_u256(user_transactions.nonce);
    } else {
        nonce = [0; 32];
    }
    let data = "0x".to_owned() + &utils::vec_u8_to_string(&bytecode);
    let tx = transaction::Transaction1559 {
        nonce,
        chain_id,
        max_priority_fee_per_gas,
        gas_limit,
        max_fee_per_gas,
        to: "0x".to_string(),
        value: [0; 32],
        data,
        access_list: vec![],
        v: "0x00".to_string(),
        r: "0x00".to_string(),
        s: "0x00".to_string(),
    };

    let raw_tx = tx.serialize().unwrap();
    let res = sign_transaction(raw_tx, chain_id, principal_id)
        .await
        .unwrap();

    Ok(DeployContractResponse { tx: res.sign_tx })
}

pub async fn transfer_erc_20(
    principal_id: Principal,
    chain_id: u64,
    max_priority_fee_per_gas: U256,
    gas_limit: U256,
    max_fee_per_gas: U256,
    address: String,
    value: U256,
    contract_address: String,
) -> Result<TransferERC20Response, String> {
    let users = STATE.with(|s| s.borrow().users.clone());
    let user;

    if let Some(i) = users.get(&principal_id) {
        user = i.clone();
    } else {
        return Err("this user does not exist".to_string());
    }

    let nonce: U256;
    if let Some(user_transactions) = user.transactions.get(&chain_id) {
        nonce = utils::u64_to_u256(user_transactions.nonce);
    } else {
        nonce = [0; 32];
    }

    let data = "0x".to_owned() + &utils::get_transfer_data(&address, value).unwrap();

    let tx = transaction::Transaction1559 {
        nonce,
        chain_id,
        max_priority_fee_per_gas,
        gas_limit,
        max_fee_per_gas,
        to: contract_address,
        value: [0; 32],
        data,
        access_list: vec![],
        v: "0x00".to_string(),
        r: "0x00".to_string(),
        s: "0x00".to_string(),
    };

    let raw_tx = tx.serialize().unwrap();

    let res = sign_transaction(raw_tx, chain_id, principal_id)
        .await
        .unwrap();

    Ok(TransferERC20Response { tx: res.sign_tx })
}

pub fn get_caller_data(principal_id: Principal, chain_id: u64) -> Option<UserResponse> {
    let users = STATE.with(|s| s.borrow().users.clone());
    let user;
    if let Some(i) = users.get(&principal_id) {
        user = i.clone();
    } else {
        return None;
    }

    let address = get_address_from_public_key(user.public_key.clone()).unwrap();

    let transaction_data = user
        .transactions
        .get(&chain_id)
        .cloned()
        .unwrap_or_else(|| TransactionChainData::default());

    Some(UserResponse {
        address,
        transactions: transaction_data,
    })
}

pub fn clear_caller_history(principal_id: Principal, chain_id: u64) -> Result<(), String> {
    let users = STATE.with(|s| s.borrow().users.clone());

    if let None = users.get(&principal_id) {
        return Err("this user does not exist".to_string());
    }

    STATE.with(|s| {
        let mut state = s.borrow_mut();
        let user = state.users.get_mut(&principal_id).unwrap();
        let user_tx = user.transactions.get_mut(&chain_id);
        if let Some(user_transactions) = user_tx {
            user_transactions.transactions.clear();
        }
    });

    Ok(())
}

pub fn pre_upgrade() {
    STATE.with(|s| {
        ic_cdk::storage::stable_save((s,)).unwrap();
    });
}

pub fn post_upgrade() {
    let (s_prev,): (State,) = ic_cdk::storage::stable_restore().unwrap();
    STATE.with(|s| {
        *s.borrow_mut() = s_prev;
    });
}
#[cfg(test)]
mod tests;
