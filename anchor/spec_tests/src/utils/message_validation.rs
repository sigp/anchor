use ssv_types::{
    OperatorId,
    consensus::{QbftMessage, QbftMessageType},
    message::SignedSSVMessage,
    msgid::MessageId,
};
use types::{VariableList, typenum::U56};

/// Extract the decided value from a SignedSSVMessage.
/// The decided value is in the full_data field.
pub fn extract_decided_value(signed_msg: &SignedSSVMessage) -> Vec<u8> {
    signed_msg.full_data().to_vec()
}

/// Convert a VariableList identifier to MessageId
pub fn identifier_to_message_id(identifier: &VariableList<u8, U56>) -> Result<MessageId, String> {
    let msg_id_bytes: Vec<u8> = identifier.iter().cloned().collect();
    if msg_id_bytes.len() != 56 {
        return Err("invalid msg: identifier has wrong length".to_string());
    }
    let mut id_array = [0u8; 56];
    id_array.copy_from_slice(&msg_id_bytes);
    Ok(MessageId::from(id_array))
}

/// Check if a message is a decided message (commit with quorum)
pub fn is_decided_message(
    qbft_msg: &QbftMessage,
    operator_ids: &[OperatorId],
    quorum_size: usize,
) -> bool {
    matches!(qbft_msg.qbft_message_type, QbftMessageType::Commit)
        && operator_ids.len() >= quorum_size
}
