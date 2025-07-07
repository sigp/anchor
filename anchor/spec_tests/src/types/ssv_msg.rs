use serde::Deserialize;
use ssv_types::msgid::{DutyExecutor, MessageId};

use crate::{
    SpecTest, SpecTestType, types::TypesSpecTestType, utils::test_keys::TESTING_VALIDATOR_PUBKEY,
};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SSVMessageTest {
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "MessageIDs")]
    pub message_ids: Vec<MessageId>,
    #[serde(rename = "BelongsToValidator")]
    pub belongs_to_validator: bool,
}

impl SpecTest for SSVMessageTest {
    fn name(&self) -> &str {
        &self.name
    }

    fn setup(&mut self) {
        // No-op
    }

    fn run(&self) -> bool {
        // Setup the 4 share set
        let mut result = true;
        for msg_id in &self.message_ids {
            // Some of message ids have an invalid role
            if let Some(duty_executor) = msg_id.duty_executor() {
                let validator_pubkey = match duty_executor {
                    DutyExecutor::Validator(key) => key,
                    _ => return false,
                };

                if self.belongs_to_validator {
                    result &= validator_pubkey == *TESTING_VALIDATOR_PUBKEY;
                } else {
                    result &= validator_pubkey != *TESTING_VALIDATOR_PUBKEY;
                }
            }
        }
        result
    }

    fn test_type() -> SpecTestType {
        SpecTestType::Types(TypesSpecTestType::SSVMsg)
    }
}
