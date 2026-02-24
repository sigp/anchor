use serde::Deserialize;
use ssv_types::msgid::{DutyExecutor, MessageId};

use crate::{
    SpecTest,
    utils::{TESTING_VALIDATOR_PUBKEY, deserializers::deserialize_hex_message_id_list},
};

/// SSVMessageTest checks if message IDs belong to the testing validator.
///
/// Go reference: `ssvmsg/test.go` -> for each MessageID, checks
/// `ValidatorPubKey.MessageIDBelongs(msgID)` against `BelongsToValidator`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct SSVMessageTest {
    #[serde(
        rename = "MessageIDs",
        deserialize_with = "deserialize_hex_message_id_list"
    )]
    message_ids: Vec<MessageId>,
    belongs_to_validator: bool,
}

impl SpecTest for SSVMessageTest {
    fn run(&self) -> Result<(), String> {
        for msg_id in &self.message_ids {
            // Extract the duty executor from the message ID.
            // Invalid roles or committee IDs can't match a validator pubkey.
            let belongs = msg_id
                .duty_executor()
                .map(|executor| match executor {
                    DutyExecutor::Validator(key) => key == *TESTING_VALIDATOR_PUBKEY,
                    DutyExecutor::Committee(_) => false,
                })
                .unwrap_or(false);

            if belongs != self.belongs_to_validator {
                return Err(format!(
                    "Expected belongs_to_validator={}, got {belongs}",
                    self.belongs_to_validator,
                ));
            }
        }
        Ok(())
    }
}
