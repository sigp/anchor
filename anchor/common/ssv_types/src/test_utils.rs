//! Test utilities shared across the ssv_types crate

use crate::{
    OperatorId, RSA_SIGNATURE_SIZE,
    message::{MsgType, SSVMessage, SignedSSVMessage},
    msgid::MessageId,
};

const IDENTIFIER_SIZE: usize = 56; // same as MessageId length

/// Returns a default 56-byte ID array with all zeros.
pub fn default_msg_id() -> MessageId {
    [0u8; IDENTIFIER_SIZE].into()
}

/// Returns a small, non-empty payload for SSVMessage data.
pub fn small_data() -> Vec<u8> {
    vec![0x11, 0x22, 0x33]
}

/// Returns a valid signature of exactly [`RSA_SIGNATURE_SIZE`] bytes.
pub fn valid_signature() -> [u8; RSA_SIGNATURE_SIZE] {
    [0u8; RSA_SIGNATURE_SIZE]
}

/// Creates a valid, non-empty SSVMessage (ensuring it doesn't exceed the max size).
pub fn valid_ssv_message() -> SSVMessage {
    SSVMessage::new(MsgType::SSVConsensusMsgType, default_msg_id(), small_data())
        .expect("Creating a valid SSVMessage must succeed")
}

/// Creates a single-signer, single-signature valid SignedSSVMessage.
pub fn valid_signed_ssv_message() -> SignedSSVMessage {
    let msg = valid_ssv_message();
    SignedSSVMessage::new(
        vec![valid_signature()],
        vec![OperatorId(1)],
        msg,
        vec![0xAB, 0xCD], // "full_data" well under max
    )
    .expect("Creating a valid SignedSSVMessage must succeed")
}
