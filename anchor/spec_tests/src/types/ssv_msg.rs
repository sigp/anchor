use crate::{SpecTest, SpecTestType, types::TypesSpecTestType, types::types_deserializers::*};
use serde::Deserialize;
use ssv_types::msgid::MessageId;

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
        // Setup any required test state
    }

    fn run(&self) -> bool {
        true
    }

    fn test_type() -> SpecTestType {
        SpecTestType::Types(TypesSpecTestType::SSVMsg)
    }
}
