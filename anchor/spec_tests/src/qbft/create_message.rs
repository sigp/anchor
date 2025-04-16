use openssl::pkey::{PKey, Private};
use serde::Deserialize;
use ssv_types::{consensus::QbftMessageType, msgid::MessageId, IndexSet, OperatorId, Round};
use types::Hash256;

use super::{qbft_deserializers::*, SpecQbft};
use crate::{qbft::SignedSSVMessage, utils::TestKeySet, QbftSpecTestType, SpecTest, SpecTestType};

impl SpecTest for CreateMessageTest {
    fn name(&self) -> &str {
        &self.name
    }

    // Run the test by constructing the message and verifying its correctness
    fn run(&self) -> bool {
        let spec_qbft = self.spec_qbft.as_ref().expect("Setup has been called");
        let key = self.signing_key.as_ref().expect("Setup has been called");
        let prepare_justifications = if let Some(prepare) = &self.prepare_justifications {
            prepare.clone()
        } else {
            Vec::new()
        };

        let round_change_justifications =
            if let Some(round_change) = &self.round_change_justifications {
                round_change.clone()
            } else {
                Vec::new()
            };

        // Create a new unsigned message. Have to create a new unsigned message to be received on
        // the queue and then perform signing
        let unsigned_message = spec_qbft.create_message(
            self.create_type,
            self.root,
            round_change_justifications,
            prepare_justifications,
        );
        let signed_message = spec_qbft.sign(unsigned_message, key);

        // Compute the merkle root of the message and compare it to the expected_root
        spec_qbft.verify_root(signed_message, self.expected_root)

        // If there are justifications, verify those.. todo!()
    }

    // Setup the qbft instance for constructing a new message
    fn setup(&mut self) {
        let four_share_set = TestKeySet::four_share_set();
        let committee: IndexSet<OperatorId> =
            four_share_set.operator_keys.keys().cloned().collect();

        // All test identifiers are [1,2,3,4]
        let identifier = MessageId::for_spectest();

        // All message creation testing code uses operator one as the message signer
        let operator_one_private = four_share_set
            .operator_keys
            .get(&OperatorId::from(1))
            .expect("Exists");
        let operator_one_private =
            PKey::from_rsa(operator_one_private.to_owned()).expect("Valid key");

        let qbft = SpecQbft::new(committee, identifier);

        // Complete the setup
        self.spec_qbft = Some(qbft);
        self.signing_key = Some(operator_one_private);
    }

    fn test_type() -> SpecTestType {
        SpecTestType::Qbft(QbftSpecTestType::CreateMessage)
    }
}

#[derive(Deserialize)]
pub struct CreateMessageTest {
    // Name of the test that is being run
    #[serde(rename = "Name")]
    pub name: String,

    // Root of the QBFT Message, This is the unhashed ssz bytes of the data
    #[serde(rename = "Value", deserialize_with = "deserialize_value_into_root")]
    pub root: Hash256,

    // The last prepared value of the qbft instance. Todo!() What format is this in??
    #[serde(rename = "StateValue")]
    pub state_value: Option<String>,

    // The round this message is for
    #[serde(rename = "Round", deserialize_with = "deserialize_u64_into_round")]
    pub round: Option<Round>,

    // Any round change justifications for the message
    #[serde(rename = "RoundChangeJustifications")]
    pub round_change_justifications: Option<Vec<SignedSSVMessage>>,

    // Any prepare justifications for the message
    #[serde(rename = "PrepareJustifications")]
    pub prepare_justifications: Option<Vec<SignedSSVMessage>>,

    // The type of the QBFT Message to create
    #[serde(
        rename = "CreateType",
        deserialize_with = "deserialize_qbft_message_type"
    )]
    pub create_type: QbftMessageType,

    // The Expected Root of the QBFT Message
    #[serde(rename = "ExpectedRoot")]
    pub expected_root: Hash256,

    // Any Errors that were expected
    #[serde(rename = "ExpectedError")]
    pub expected_error: String,

    // Qbft Instance that is used for running the test. Skip this during deserialization
    #[serde(skip)]
    pub spec_qbft: Option<SpecQbft>,

    // The operator private key for message signing
    #[serde(skip)]
    pub signing_key: Option<PKey<Private>>,
}
