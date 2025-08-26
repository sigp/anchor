use serde::Deserialize;
use ssv_types::{consensus::QbftMessage, message::SignedSSVMessage};
use ssz::{Decode, Encode};
use tree_hash::TreeHash;
use types::Hash256;

use crate::{
    QbftSpecTestType, SpecTest, SpecTestType,
    adapters::spec_types::TestSignedSSVMessage,
    utils::{
        deserializers::{deserialize_base64_list_option, deserialize_hash256_list_option},
        error_mapping::{QbftMessageError, map_qbft_message_error},
    },
};

#[derive(Deserialize)]
pub struct QbftMessageTest {
    #[serde(rename = "Name")]
    pub name: String,

    #[serde(rename = "Type")]
    pub test_type: String,

    #[serde(rename = "Documentation")]
    pub documentation: String,

    #[serde(rename = "Messages")]
    pub messages: Vec<TestSignedSSVMessage>,

    #[serde(
        rename = "EncodedMessages",
        deserialize_with = "deserialize_base64_list_option"
    )]
    pub encoded_messages: Option<Vec<Vec<u8>>>,

    #[serde(
        rename = "ExpectedRoots",
        deserialize_with = "deserialize_hash256_list_option"
    )]
    pub expected_roots: Option<Vec<Hash256>>,

    #[serde(rename = "ExpectedError")]
    pub expected_error: String,
}

impl SpecTest for QbftMessageTest {
    fn run(&self) -> bool {
        let mut test_error: Option<QbftMessageError> = None;

        for (i, test_message) in self.messages.iter().enumerate() {
            let message: SignedSSVMessage = match test_message.clone().try_into() {
                Ok(msg) => msg,
                Err(e) => {
                    test_error = Some(QbftMessageError::ConversionError(e));
                    continue;
                }
            };

            if let Err(e) = message.validate() {
                test_error = Some(QbftMessageError::SignedMessageError(e));
                continue;
            }

            // make sure we can decode the message
            let qbft_message = match QbftMessage::from_ssz_bytes(message.ssv_message().data()) {
                Ok(msg) => msg,
                Err(e) => {
                    test_error = Some(QbftMessageError::SSZDecodeError(e));
                    continue;
                }
            };

            if let Err(e) = qbft_message.validate() {
                test_error = Some(QbftMessageError::Validation(e));
                continue;
            }

            if let Some(ref encoded_messages) = self.encoded_messages {
                if !encoded_messages.is_empty() {
                    let encoded = message.as_ssz_bytes();
                    if encoded_messages[i] != encoded {
                        return false;
                    }
                }
            }

            if let Some(ref expected_roots) = self.expected_roots {
                if !expected_roots.is_empty() {
                    let root = message.tree_hash_root();
                    if expected_roots[i] != root {
                        return false;
                    }
                }
            }
        }

        if !self.expected_error.is_empty() {
            match test_error {
                Some(ref error) => map_qbft_message_error(error) == self.expected_error,
                None => false,
            }
        } else {
            // Test expects no error
            test_error.is_none()
        }
    }

    fn test_type() -> SpecTestType {
        SpecTestType::Qbft(QbftSpecTestType::QbftMessage)
    }
}
