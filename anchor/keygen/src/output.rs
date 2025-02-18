use crate::{cli::SharedKeygenOptions, util::serialize_rsa};
use crate::{EncryptedKeyShare, ValidatorKeys};
use alloy::primitives::Keccak256;
use base64::prelude::*;
use hex::FromHex;
use openssl::pkey::Public;
use openssl::rsa::Rsa;
use serde::Serialize;
use types::{Address, Hash256, PublicKey};

#[derive(Debug, Serialize)]
pub struct OutputData {
    // tood!() version
    // todo!() created at
    shares: Vec<OutputKeyShare>,
}

#[derive(Debug, Serialize)]
struct OutputKeyShare {
    pub data: OutputKeyData,
    pub payload: Payload,
}

#[derive(Debug, Serialize)]
pub struct Payload {
    public_key: PublicKey,
    operator_ids: Vec<u64>,
    shares_data: String,
}

#[derive(Debug, Serialize)]
struct OutputKeyData {
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

pub fn encrypted_to_output(
    encrypted_keys: Vec<EncryptedKeyShare>,
    shared: SharedKeygenOptions,
    keys: ValidatorKeys,
    nonce: u64,
) -> OutputData {
    let payload = construct_payload(&encrypted_keys, &shared, &keys, nonce, shared.owner);

    let mut operators = Vec::new();
    for encrypted in encrypted_keys {
        let operator = Operator {
            id: encrypted.id,
            public_key: encrypted.public_key,
        };

        operators.push(operator);
    }

    // output key data with the shared here
    let output_key_data = OutputKeyData {
        owner_nonce: 10,
        owner_address: shared.owner,
        public_key: keys.public_key,
        operators,
    };

    let output_key_share = OutputKeyShare {
        data: output_key_data,
        payload,
    };

    OutputData {
        shares: vec![output_key_share],
    }
}

// [signature | public keys | encrypted keys].
pub fn construct_payload(
    encrypted_keys: &[EncryptedKeyShare],
    shared: &SharedKeygenOptions,
    keys: &ValidatorKeys,
    nonce: u64,
    owner: Address,
) -> Payload {
    // Construct the unique owner signature
    let message = format!("{}:{}", owner, nonce);

    let mut hasher = Keccak256::new();
    hasher.update(message.as_bytes());
    let hash = hasher.finalize();

    let signature = keys.secret_key.sign(hash);

    // Join together all of the public keyhs and the encrypyed keys
    let mut pk_concat = String::new();
    let mut encrypted_concat = String::new();
    let mut ids = vec![];

    for key in encrypted_keys {
        let serialized_key = key.public_key.public_key_to_pem().unwrap();
        let encoded = BASE64_STANDARD.encode(serialized_key.clone());
        pk_concat.push_str(&encoded);

        ids.push(key.id);

        let encoded = BASE64_STANDARD.encode(key.encrypted_keyshare.clone());
        encrypted_concat.push_str(&encoded);
    }

    let output_payload = format!("0x{}{}{}", signature, pk_concat, encrypted_concat);
    Payload {
        public_key: keys.public_key.clone(),
        operator_ids: ids,
        shares_data: output_payload,
    }
}
