use openssl::pkey::{PKey, Private};
use serde::Deserialize;
use ssv_types::{IndexSet, OperatorId, Round, consensus::QbftMessageType, msgid::MessageId};
use ssz::Decode;
use tree_hash::TreeHash;
use types::Hash256;

use super::{SpecQbft, qbft_deserializers::*};
use crate::{
    QbftSpecTestType, SpecTest, SpecTestType, qbft::SignedSSVMessage, utils::test_keys::TestKeySet,
};

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
            self.round,
            round_change_justifications,
            prepare_justifications,
        );

        let signed_message = spec_qbft.sign(unsigned_message, key);

        // Compute the merkle root of the message and compare it to the expected_root
        let result = spec_qbft.verify_root(signed_message.clone(), self.expected_root);

        // If verification failed, load and compare with Go final state
        if !result {
            println!("\n❌ FAILED - Test '{}'", self.name);
            println!(
                "   Rust hash: {}",
                hex::encode(signed_message.tree_hash_root())
            );
            println!("   Expected:  {}", hex::encode(self.expected_root));

            self.compare_with_go_final_state(&signed_message);
        } else {
            println!("✅ PASSED - Test '{}'", self.name);
        }

        result

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

// Representation of CreateMsgSpecTest files
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
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

impl CreateMessageTest {
    fn compare_with_go_final_state(&self, rust_msg: &SignedSSVMessage) {
        // Try to load the corresponding Go final state file
        let go_final_state_path = self.get_go_final_state_path();

        match std::fs::read_to_string(&go_final_state_path) {
            Ok(go_json) => match serde_json::from_str::<CreateMessageTest>(&go_json) {
                Ok(go_final_state) => {
                    self.detailed_comparison(&go_final_state, rust_msg, &go_final_state_path);
                }
                Err(e) => {
                    println!("   ❌ Failed to parse Go final state JSON: {e}");
                }
            },
            Err(_e) => {
                println!("   ❌ No Go comparison state available");
            }
        }
    }

    fn get_original_test_file_path(&self) -> String {
        // Convert test name to the original Go test file name format
        let sanitized_name = self.name.replace(" ", "_");
        format!(
            "src/ssv-spec/qbft/spectest/generate/tests/tests.CreateMsgSpecTest_qbft_create_message_{sanitized_name}.json"
        )
    }

    fn get_go_final_state_path(&self) -> String {
        // Convert test name to the Go file name format
        let sanitized_name = self.name.clone(); // Go uses spaces in filenames
        format!(
            "src/ssv-spec/qbft/spectest/generate/state_comparison/tests_CreateMsgSpecTest/qbft create message {sanitized_name}.json"
        )
    }

    fn list_available_go_files(&self) {
        let dir_path =
            "src/ssv-spec/qbft/spectest/generate/state_comparison/tests_CreateMsgSpecTest/";
        if let Ok(entries) = std::fs::read_dir(dir_path) {
            println!("Available Go state files:");
            for entry in entries.flatten() {
                if let Some(filename) = entry.file_name().to_str() {
                    if filename.ends_with(".json") {
                        println!("  - {filename}");
                    }
                }
            }
        }

        // Also try direct name mapping
        let direct_path = format!(
            "src/ssv-spec/qbft/spectest/generate/state_comparison/tests_CreateMsgSpecTest/{}.json",
            self.name
        );
        println!("Also tried: {direct_path}");
    }

    fn detailed_comparison(
        &self,
        go_state: &CreateMessageTest,
        rust_msg: &SignedSSVMessage,
        _go_file_path: &str,
    ) {
        // Decode Rust QBFT message
        if let Ok(rust_qbft_msg) =
            ssv_types::consensus::QbftMessage::from_ssz_bytes(rust_msg.ssv_message().data())
        {
            let mut mismatches = Vec::new();

            // Check key fields for mismatches

            // Message Type
            let go_create_type_str = format!("{:?}", go_state.create_type);
            let rust_msg_type_str = format!("{:?}", rust_qbft_msg.qbft_message_type);
            if rust_msg_type_str != go_create_type_str {
                mismatches.push(format!(
                    "QbftMessage.msg_type: Rust={rust_msg_type_str} vs Go={go_create_type_str}"
                ));
            }

            // Round comparison
            let go_round = go_state.round.map(u64::from).unwrap_or(0);
            let rust_round = rust_qbft_msg.round;
            if rust_round != go_round {
                mismatches.push(format!(
                    "QbftMessage.round: Rust={rust_round} vs Go={go_round}"
                ));
            }

            // Height (should be 0 for tests)
            if rust_qbft_msg.height != 0 {
                mismatches.push(format!(
                    "QbftMessage.height: Rust={} vs Go=0",
                    rust_qbft_msg.height
                ));
            }

            // Root comparison
            if rust_qbft_msg.root != go_state.root {
                mismatches.push(format!(
                    "QbftMessage.root: Rust={} vs Go={}",
                    hex::encode(rust_qbft_msg.root),
                    hex::encode(go_state.root)
                ));
            }

            // FullData comparison
            let go_full_data_len = go_state
                .round_change_justifications
                .as_ref()
                .and_then(|rcs| rcs.first())
                .map(|rc| rc.full_data().len())
                .unwrap_or(0);

            if rust_msg.full_data().len() != go_full_data_len && go_full_data_len > 0 {
                mismatches.push(format!(
                    "SignedSSVMessage.full_data.length: Rust={} vs Go={}",
                    rust_msg.full_data().len(),
                    go_full_data_len
                ));
            }

            // Justifications count
            let go_rc_count = go_state
                .round_change_justifications
                .as_ref()
                .map(|v| v.len())
                .unwrap_or(0);
            let go_prep_count = go_state
                .prepare_justifications
                .as_ref()
                .map(|v| v.len())
                .unwrap_or(0);

            if rust_qbft_msg.round_change_justification.len() != go_rc_count {
                mismatches.push(format!(
                    "QbftMessage.round_change_justification.length: Rust={} vs Go={}",
                    rust_qbft_msg.round_change_justification.len(),
                    go_rc_count
                ));
            }

            if rust_qbft_msg.prepare_justification.len() != go_prep_count {
                mismatches.push(format!(
                    "QbftMessage.prepare_justification.length: Rust={} vs Go={}",
                    rust_qbft_msg.prepare_justification.len(),
                    go_prep_count
                ));
            }

            // Show mismatches if any
            if !mismatches.is_empty() {
                println!("\n🔍 KEY MISMATCHES DETECTED:");
                println!("   📋 Structure: SignedSSVMessage → SSVMessage → QbftMessage");
                for mismatch in mismatches {
                    println!("   ❌ {mismatch}");
                }
            }

            // Check for specific FullData mismatches in justifications
            self.check_justification_mismatches(&rust_qbft_msg, go_state);
        } else {
            println!("   ❌ Failed to decode Rust QBFT message from SSVMessage data");
        }
    }

    fn check_justification_mismatches(
        &self,
        rust_qbft_msg: &ssv_types::consensus::QbftMessage,
        go_state: &CreateMessageTest,
    ) {
        // Check RoundChange justifications for FullData mismatches
        if let Some(go_rc_justifications) = &go_state.round_change_justifications {
            let mut rc_mismatches = 0;
            for (rust_rc_bytes, go_rc) in rust_qbft_msg
                .round_change_justification
                .iter()
                .zip(go_rc_justifications.iter())
            {
                if let Ok(rust_rc) = SignedSSVMessage::from_ssz_bytes(rust_rc_bytes) {
                    if rust_rc.full_data().len() != go_rc.full_data().len() {
                        rc_mismatches += 1;
                    }
                }
            }
            if rc_mismatches > 0 {
                println!(
                    "   ❌ QbftMessage.round_change_justification[*].full_data.length: {rc_mismatches} items have mismatches"
                );
            }
        }

        // Check Prepare justifications for FullData mismatches
        if let Some(go_prep_justifications) = &go_state.prepare_justifications {
            let mut prep_mismatches = 0;
            for (rust_prep_bytes, go_prep) in rust_qbft_msg
                .prepare_justification
                .iter()
                .zip(go_prep_justifications.iter())
            {
                if let Ok(rust_prep) = SignedSSVMessage::from_ssz_bytes(rust_prep_bytes) {
                    if rust_prep.full_data().len() != go_prep.full_data().len() {
                        prep_mismatches += 1;
                    }
                }
            }
            if prep_mismatches > 0 {
                println!(
                    "   ❌ QbftMessage.prepare_justification[*].full_data.length: {prep_mismatches} items have mismatches"
                );
            }
        }
    }
}
