use crate::{
    cli::SharedKeygenOptions,
    util::serialize_rsa,
    {EncryptedKeyShare, ValidatorKeys},
};
use alloy::primitives::Keccak256;
use chrono::{DateTime, Utc};
use openssl::{pkey::Public, rsa::Rsa};
use serde::Serialize;
use types::{Address, PublicKey};

const VERSION: &str = "v1.2.1";

#[derive(Debug, Serialize)]
pub struct OutputData {
    version: String,
    #[serde(rename = "createdAt")]
    created_at: DateTime<Utc>,
    shares: Vec<OutputKeyShare>,
}

#[derive(Debug, Serialize)]
struct OutputKeyShare {
    data: OutputKeyData,
    payload: Payload,
}

#[derive(Debug, Serialize)]
pub struct Payload {
    #[serde(rename = "publicKey")]
    public_key: PublicKey,
    #[serde(rename = "operatorIds")]
    operator_ids: Vec<u64>,
    #[serde(rename = "sharesData")]
    shares_data: String,
}

#[derive(Debug, Serialize)]
struct OutputKeyData {
    #[serde(rename = "ownerNonce")]
    owner_nonce: u64,
    #[serde(rename = "ownerAddress")]
    owner_address: Address,
    #[serde(rename = "publicKey")]
    public_key: PublicKey,
    operators: Vec<Operator>,
}

#[derive(Debug, Serialize)]
struct Operator {
    id: u64,
    #[serde(serialize_with = "serialize_rsa", rename = "operatorKey")]
    public_key: Rsa<Public>,
}

impl From<EncryptedKeyShare> for Operator {
    fn from(encrypted: EncryptedKeyShare) -> Self {
        Self {
            id: encrypted.id,
            public_key: encrypted.public_key,
        }
    }
}

impl OutputData {
    pub fn new(
        encrypted_keys: Vec<EncryptedKeyShare>,
        shared: SharedKeygenOptions,
        keys: ValidatorKeys,
        nonce: u64,
    ) -> Self {
        let payload = Payload::new(&encrypted_keys, &keys, nonce, shared.owner);
        let operators: Vec<Operator> = encrypted_keys.into_iter().map(Operator::from).collect();

        let output_key_data = OutputKeyData {
            owner_nonce: nonce,
            owner_address: shared.owner,
            public_key: keys.public_key,
            operators,
        };

        Self {
            version: VERSION.to_string(),
            created_at: Utc::now(),
            shares: vec![OutputKeyShare {
                data: output_key_data,
                payload,
            }],
        }
    }
}

impl Payload {
    pub fn new(
        encrypted_keys: &[EncryptedKeyShare],
        keys: &ValidatorKeys,
        nonce: u64,
        owner: Address,
    ) -> Self {
        let signature = Self::create_signature(keys, nonce, owner);
        let (public_keys, encrypted_data) = Self::concatenate_key_data(encrypted_keys);
        let operator_ids: Vec<u64> = encrypted_keys.iter().map(|key| key.id).collect();

        Self {
            public_key: keys.public_key.clone(),
            operator_ids,
            shares_data: format!("0x{}{}{}", signature, public_keys, encrypted_data),
        }
    }

    // Creates a signature with the owner address and the nonce
    fn create_signature(keys: &ValidatorKeys, nonce: u64, owner: Address) -> String {
        let message = format!("{}:{}", owner, nonce);
        let mut hasher = Keccak256::new();
        hasher.update(message.as_bytes());

        let signature = keys.secret_key.sign(hasher.finalize());
        hex::encode(signature.serialize())
    }

    // Concatenates together all of the share public keys and the encrypted keyshares for the
    // payload
    fn concatenate_key_data(encrypted_keys: &[EncryptedKeyShare]) -> (String, String) {
        let mut public_keys = String::new();
        let mut encrypted_data = String::new();

        for key in encrypted_keys {
            public_keys.push_str(&hex::encode(key.share_public_key.serialize()));
            encrypted_data.push_str(&hex::encode(&key.encrypted_keyshare));
        }

        (public_keys, encrypted_data)
    }
}
