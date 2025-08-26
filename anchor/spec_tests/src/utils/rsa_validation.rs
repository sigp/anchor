use openssl::{hash::MessageDigest, pkey::PKey, sign::Verifier};
use qbft::WrappedQbftMessage;
use ssv_types::{OperatorId, consensus::QbftMessageType, message::SignedSSVMessage};
use ssz::{Decode, Encode};

use crate::utils::test_keys::TestKeySet;

/// Validate RSA signatures for justifications in QBFT messages
/// In production, message_validator does this. In tests, we need to do it here.
pub fn validate_rsa_signatures(
    wrapped: &WrappedQbftMessage,
    test_keys: &TestKeySet,
) -> Result<(), String> {
    let msg_type = wrapped.qbft_message.qbft_message_type;

    // Only validate justifications for proposals and round changes
    if msg_type != QbftMessageType::Proposal && msg_type != QbftMessageType::RoundChange {
        return Ok(());
    }

    // Validate round change justification signatures only
    for rc_bytes in &wrapped.qbft_message.round_change_justification {
        let rc_msg = SignedSSVMessage::from_ssz_bytes(rc_bytes).map_err(|_| {
            if msg_type == QbftMessageType::Proposal {
                "invalid signed message: proposal not justified: change round msg not valid: decode failed".to_string()
            } else {
                "invalid signed message: round change justification invalid: decode failed".to_string()
            }
        })?;

        // Only check RSA signatures - let core handle all protocol validation
        for (&op_id, sig) in rc_msg.operator_ids().iter().zip(rc_msg.signatures().iter()) {
            // Convert signature from VariableList to [u8; 256]
            if sig.len() != 256 {
                return Err("invalid signed message: invalid signature length".to_string());
            }
            let mut sig_array = [0u8; 256];
            sig_array.copy_from_slice(&sig[..]);

            if !verify_rsa_signature(
                rc_msg.ssv_message().as_ssz_bytes(),
                op_id,
                &sig_array,
                test_keys,
            ) {
                // Different error messages based on parent message type
                if msg_type == QbftMessageType::Proposal {
                    return Err("invalid signed message: proposal not justified: change round msg not valid: msg signature invalid: crypto/rsa: verification error".to_string());
                } else {
                    return Err("invalid signed message: round change justification invalid: msg signature invalid: crypto/rsa: verification error".to_string());
                }
            }
        }
    }

    // Validate prepare justification signatures only
    for prepare_bytes in &wrapped.qbft_message.prepare_justification {
        let prepare_msg = SignedSSVMessage::from_ssz_bytes(prepare_bytes).map_err(|_| {
            "invalid signed message: prepare justification invalid: decode failed".to_string()
        })?;

        // Only check RSA signatures - let core handle all protocol validation
        for (&op_id, sig) in prepare_msg
            .operator_ids()
            .iter()
            .zip(prepare_msg.signatures().iter())
        {
            // Convert signature from VariableList to [u8; 256]
            if sig.len() != 256 {
                return Err("invalid signed message: invalid signature length".to_string());
            }
            let mut sig_array = [0u8; 256];
            sig_array.copy_from_slice(&sig[..]);

            if !verify_rsa_signature(
                prepare_msg.ssv_message().as_ssz_bytes(),
                op_id,
                &sig_array,
                test_keys,
            ) {
                return Err("invalid signed message: prepare justification invalid: msg signature invalid: crypto/rsa: verification error".to_string());
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
