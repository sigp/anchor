use crate::util::serialize_rsa;
use crate::EncryptedKeyShare;
use openssl::pkey::Public;
use openssl::rsa::Rsa;
use serde::Serialize;
use types::{Address, PublicKey};

#[derive(Debug, Serialize)]
pub struct OutputData {
    // tood!() version
    // todo!() created at
    shares: Vec<KeyShare>,
}

#[derive(Debug, Serialize)]
struct KeyShare {
    pub data: KeyData,
    pub payload: Payload,
}

#[derive(Debug, Serialize)]
struct Payload {
    public_key: PublicKey,
    operator_ids: Vec<u64>,
    shares_data: Vec<u8>,
}

#[derive(Debug, Serialize)]
struct KeyData {
    owner_nonce: u64,
    owner_address: Address,
    public_key: PublicKey,
    operators: Vec<Operator>,
}

#[derive(Debug, Serialize)]
struct Operator {
    id: u64,
    #[serde(serialize_with = "serialize_rsa")]
    public_key: Rsa<Public>,
}


pub fn encrypted_to_output(encrypted_keys: Vec<EncryptedKeyShare>) -> OutputData {
    todo!()
}
