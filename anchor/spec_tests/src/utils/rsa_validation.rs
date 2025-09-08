use openssl::{hash::MessageDigest, pkey::PKey, sign::Verifier};
use qbft::WrappedQbftMessage;
use ssv_types::{OperatorId, consensus::QbftMessageType, message::SignedSSVMessage};
use ssz::Decode;

use crate::utils::test_keys::TestKeySet;

/// Validate RSA signatures for QBFT messages
/// In production, message_validator does this. In tests, we need to do it here.
pub fn validate_rsa_signatures(
    wrapped: &WrappedQbftMessage,
    test_keys: &TestKeySet,
) -> Result<(), Vec<String>> {
    if let Err(_) = test_keys.verify_signed_messages(&[wrapped.signed_message.clone()]) {
        // Sigs for tests are valid, so if this failed then it is the test case where the signer
        // is not in the committee
        return Err(vec![
            "invalid signed message: signer not in committee".to_string(),
        ]);
    }
    let msg_type = wrapped.qbft_message.qbft_message_type;

    // Validate round change justification signatures only
    for rc_bytes in &wrapped.qbft_message.round_change_justification {
        let rc_msg = SignedSSVMessage::from_ssz_bytes(rc_bytes).expect("Valid message");
        if let Err(_) = test_keys.verify_signed_messages(&[rc_msg.clone()]) {
            if msg_type == QbftMessageType::Proposal {
                return Err(vec!["invalid signed message: proposal not justified: change round msg not valid: msg signature invalid: crypto/rsa: verification error".to_string()]);
            } else {
                return Err(vec!["invalid signed message: round change justification invalid: msg signature invalid: crypto/rsa: verification error".to_string()]);
            }
        }
    }

    Ok(())
}

/// Verify a single RSA signature
pub fn verify_rsa_signature(
    msg_bytes: Vec<u8>,
    operator_id: OperatorId,
    signature: &[u8; 256],
    test_keys: &TestKeySet,
) -> bool {
    let Some(rsa_key) = test_keys.operator_keys.get(&operator_id) else {
        return false;
    };

    let Ok(pkey) = PKey::from_rsa(rsa_key.clone()) else {
        return false;
    };

    let Ok(mut verifier) = Verifier::new(MessageDigest::sha256(), &pkey) else {
        return false;
    };

    if verifier.update(&msg_bytes).is_err() {
        return false;
    }

    matches!(verifier.verify(signature), Ok(true))
}
