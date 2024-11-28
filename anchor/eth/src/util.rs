use super::sync::MAX_OPERATORS;
use alloy::primitives::{keccak256, Address, Bytes, FixedBytes, U256};
use std::collections::HashSet;
use types::{PublicKey};

// Offsets to parse the share bytes
const SIG_LEN: usize = 96;
const PUBKEY_LEN: usize = 48;
const ENCRYPTEDKEY_LEN: usize = 32;

// use types::(PublicKey, PrivateKey),
pub struct SharePublickKey([u8; 48]);
pub struct SharePrivateKey([u8; 32]);

// All of the public keys and encrypted private keys for a
// validator key that has been broken into N shares.
pub struct ShareKeys {
    // Uncompressed bls signatures
    signature: [u8; 96],
    // Public keys of the Shares
    public_keys: Vec<SharePublickKey>,
    // Encrypted private key of the shares
    encrypted_keys: Vec<SharePrivateKey>,
}

// Convert from a raw stream of bytes to a structured set of keys.
// Event contains a bytes stream of the form
// [signature | public keys | encrypted keys].
impl TryFrom<Bytes> for ShareKeys {
    type Error = String;
    fn try_from(source: Bytes) -> Result<ShareKeys, Self::Error> {
        todo!()
    }
}

// Verify that the signature over the share data is correct
pub fn verify_signature() -> Result<(), String> {
    todo!()
}

// Compute the unique hash of a committee when identified by an owner
pub fn compute_cluster_id(owner: Address, operator_ids: &mut [u64]) -> FixedBytes<32> {
    operator_ids.sort();

    // Concat to form <owner><id1><id2>...
    let mut byte_repr = Bytes::new();
    for id in operator_ids {}
    keccak256(byte_repr)
}

// Perform basic verification on the operator set
pub fn validate_operators(operator_ids: Vec<u64>) -> Result<(), String> {
    let num_operators = operator_ids.len();

    // make sure there is a valid number of operators
    if num_operators > MAX_OPERATORS {
        return Err(format!(
            "Validator has too many operators: {}",
            num_operators
        ));
    }
    if num_operators == 0 {
        return Err("Validator has no operators".to_string());
    }

    // make sure count is valid
    let threshold = (num_operators - 1) / 3;
    if (num_operators - 1) % 3 != 0 || !(1..=4).contains(&threshold) {
        return Err(format!("Invalid number of operators: {}", num_operators));
    }

    // make sure there are no duplicates
    let mut seen = HashSet::new();
    let are_duplicates = !operator_ids.iter().all(|x| seen.insert(x));
    if are_duplicates {
        return Err("Operator IDs contain duplicates".to_string());
    }

    Ok(())
}
