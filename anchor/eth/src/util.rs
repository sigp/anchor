use super::sync::MAX_OPERATORS;
use ssv_types::Share;
use ssv_types::{OperatorId, ValidatorMetadata};
use std::collections::HashSet;
use types::PublicKey;

const SIGNATURE_LENGTH: usize = 96; // phase0.SignatureLength
const PUBLIC_KEY_LENGTH: usize = 48; // phase0.PublicKeyLength
const ENCRYPTED_KEY_LENGTH: usize = 256; // Original encryptedKeyLength

// Validates and parses shares from a validator added event
// Event contains a bytes stream of the form
// [signature | public keys | encrypted keys].
pub fn parse_shares(
    shares: Vec<u8>,
    operator_ids: &[OperatorId],
) -> Result<(Vec<u8>, Vec<Share>), String> {
    let operator_count = operator_ids.len();

    // Calculate offsets for different components within the shares
    let signature_offset = SIGNATURE_LENGTH;
    let pub_keys_offset = PUBLIC_KEY_LENGTH * operator_count + signature_offset;
    let shares_expected_length = ENCRYPTED_KEY_LENGTH * operator_count + pub_keys_offset;

    // Validate total length of shares
    if shares_expected_length != shares.len() {
        todo!()
    }

    // Extract components using array slicing
    let signature = shares[..signature_offset].to_vec();
    let share_public_keys = split_bytes(
        &shares[signature_offset..pub_keys_offset],
        PUBLIC_KEY_LENGTH,
    );
    let encrypted_keys = split_bytes(&shares[pub_keys_offset..], ENCRYPTED_KEY_LENGTH);

    let shares: Vec<Share> = share_public_keys
        .iter()
        .zip(encrypted_keys.iter())
        .map(|(_public, _encrypted)| {
            todo!()
            /*
            Share {
                share_pubkey:  PublicKey::try_from(public),
                encrypted_private_key: encrypted.as_slice()
            }
            */
        })
        .collect();

    Ok((signature, shares))
}

// Splits a byte slice into chunks of specified size
fn split_bytes(data: &[u8], chunk_size: usize) -> Vec<Vec<u8>> {
    data.chunks(chunk_size)
        .map(|chunk| chunk.to_vec())
        .collect()
}

// Fetch the metadata for a validator from the beacon chain
pub fn fetch_validator_metadata(_public_key: PublicKey) -> ValidatorMetadata {
    todo!()
}

// Verify that the signature over the share data is correct
pub fn verify_signature(_signature: Vec<u8>) -> bool {
    todo!()
}

// Perform basic verification on the operator set
pub fn validate_operators(operator_ids: &[OperatorId]) -> Result<(), String> {
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
        return Err(format!(
            "Given {} operators. Cannot build a 3f+1 quorum",
            num_operators
        ));
    }

    // make sure there are no duplicates
    let mut seen = HashSet::new();
    let are_duplicates = !operator_ids.iter().all(|x| seen.insert(x));
    if are_duplicates {
        return Err("Operator IDs contain duplicates".to_string());
    }

    Ok(())
}
