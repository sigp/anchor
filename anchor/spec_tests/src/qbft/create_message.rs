use openssl::pkey::{PKey, Private};
use serde::Deserialize;
use ssv_types::{consensus::QbftMessageType, msgid::MessageId, IndexSet, OperatorId, Round};
use ssz::Decode;
use tree_hash::TreeHash;
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
            self.round,
            round_change_justifications,
            prepare_justifications,
        );

        let signed_message = spec_qbft.sign(unsigned_message, key);

        // Compute the merkle root of the message and compare it to the expected_root
        let result = spec_qbft.verify_root(signed_message.clone(), self.expected_root);

        // If verification failed, load and compare with Go final state
        if !result {
            println!("=== QBFT MESSAGE DEBUG ===");
            println!("Test name: {}", self.name);
            let qbft_message = ssv_types::consensus::QbftMessage::from_ssz_bytes(
                signed_message.ssv_message().data(),
            )
            .unwrap();
            println!("Message type: {:?}", qbft_message.qbft_message_type);
            println!("Round: {}", qbft_message.round);
            println!("Data round: {}", qbft_message.data_round);
            println!("Root: {}", hex::encode(qbft_message.root));

            // Also print full message encoding
            println!("=== FULL MESSAGE ENCODING ===");
            println!(
                "Rust computed hash: {}",
                hex::encode(signed_message.tree_hash_root())
            );
            println!("Expected hash from Go: {}", hex::encode(self.expected_root));

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
        println!("\n❌ FAILED - Test '{}'", self.name);
        println!("=== DETAILED COMPARISON WITH GO FINAL STATE ===");

        // Try to load the corresponding Go final state file
        let go_final_state_path = self.get_go_final_state_path();

        match std::fs::read_to_string(&go_final_state_path) {
            Ok(go_json) => match serde_json::from_str::<CreateMessageTest>(&go_json) {
                Ok(go_final_state) => {
                    println!("✓ Loaded Go final state from: {}", go_final_state_path);
                    self.detailed_comparison(&go_final_state, rust_msg, &go_final_state_path);
                }
                Err(e) => {
                    println!("✗ Failed to parse Go final state JSON: {}", e);
                    println!("Raw Go JSON:\n{}", go_json);
                }
            },
            Err(e) => {
                println!(
                    "✗ Failed to load Go final state from {}: {}",
                    go_final_state_path, e
                );
                println!("Available similar files:");
                self.list_available_go_files();
            }
        }
    }

    fn get_original_test_file_path(&self) -> String {
        // Convert test name to the original Go test file name format
        let sanitized_name = self.name.replace(" ", "_");
        format!("src/ssv-spec/qbft/spectest/generate/tests/tests.CreateMsgSpecTest_qbft_create_message_{}.json", sanitized_name)
    }

    fn get_go_final_state_path(&self) -> String {
        // Convert test name to the Go file name format
        let sanitized_name = self.name.replace(" ", " "); // Go uses spaces in filenames
        format!("src/ssv-spec/qbft/spectest/generate/state_comparison/tests_CreateMsgSpecTest/qbft create message {}.json", sanitized_name)
    }

    fn list_available_go_files(&self) {
        let dir_path =
            "src/ssv-spec/qbft/spectest/generate/state_comparison/tests_CreateMsgSpecTest/";
        if let Ok(entries) = std::fs::read_dir(dir_path) {
            println!("Available Go state files:");
            for entry in entries.flatten() {
                if let Some(filename) = entry.file_name().to_str() {
                    if filename.ends_with(".json") {
                        println!("  - {}", filename);
                    }
                }
            }
        }

        // Also try direct name mapping
        let direct_path = format!(
            "src/ssv-spec/qbft/spectest/generate/state_comparison/tests_CreateMsgSpecTest/{}.json",
            self.name
        );
        println!("Also tried: {}", direct_path);
    }

    fn detailed_comparison(
        &self,
        go_state: &CreateMessageTest,
        rust_msg: &SignedSSVMessage,
        go_file_path: &str,
    ) {
        println!("\n=== RUST vs GO FINAL STATE COMPARISON ===");
        println!("Original test file: {}", self.get_original_test_file_path());
        println!("Go final state file: {}", go_file_path);

        // Decode Rust QBFT message
        if let Ok(rust_qbft_msg) =
            ssv_types::consensus::QbftMessage::from_ssz_bytes(rust_msg.ssv_message().data())
        {
            println!("\n=== MESSAGE STRUCTURE HIERARCHY ===");
            println!("SignedSSVMessage");
            println!("├── Signatures: {} items", rust_msg.signatures().len());
            println!("├── OperatorIDs: {:?}", rust_msg.operator_ids());
            println!("├── FullData: {} bytes", rust_msg.full_data().len());
            println!("└── SSVMessage");
            println!("    ├── MsgType: {:?}", rust_msg.ssv_message().msg_type());
            println!(
                "    ├── MsgID: {}",
                hex::encode(rust_msg.ssv_message().msg_id())
            );
            println!(
                "    ├── Data: {} bytes",
                rust_msg.ssv_message().data().len()
            );
            println!("    └── QbftMessage (decoded from Data)");
            println!("        ├── Type: {:?}", rust_qbft_msg.qbft_message_type);
            println!("        ├── Height: {}", rust_qbft_msg.height);
            println!("        ├── Round: {}", rust_qbft_msg.round);
            println!("        ├── DataRound: {}", rust_qbft_msg.data_round);
            println!("        ├── Root: {}", hex::encode(rust_qbft_msg.root));
            println!(
                "        ├── RoundChangeJustifications: {} items",
                rust_qbft_msg.round_change_justification.len()
            );
            println!(
                "        └── PrepareJustifications: {} items",
                rust_qbft_msg.prepare_justification.len()
            );

            println!("\n=== FIELD-BY-FIELD COMPARISON ===");

            // SignedSSVMessage level comparison
            println!("\n--- SignedSSVMessage Level ---");
            println!("✓ Signatures count: {}", rust_msg.signatures().len());
            println!("✓ OperatorIDs: {:?}", rust_msg.operator_ids());
            println!("✓ FullData length: {} bytes", rust_msg.full_data().len());

            // SSVMessage level comparison
            println!("\n--- SSVMessage Level ---");
            println!("✓ MsgType: {:?}", rust_msg.ssv_message().msg_type());
            println!("✓ MsgID: {}", hex::encode(rust_msg.ssv_message().msg_id()));
            println!(
                "✓ Data length: {} bytes",
                rust_msg.ssv_message().data().len()
            );

            // QbftMessage level comparison
            println!("\n--- QbftMessage Level ---");

            // Message Type
            let go_create_type_str = format!("{:?}", go_state.create_type);
            let rust_msg_type_str = format!("{:?}", rust_qbft_msg.qbft_message_type);
            if rust_msg_type_str != go_create_type_str {
                println!(
                    "❌ Type:       Rust={} vs Go={}",
                    rust_msg_type_str, go_create_type_str
                );
            } else {
                println!("✓ Type:       {}", rust_msg_type_str);
            }

            // Height (should be 0 for tests)
            if rust_qbft_msg.height != 0 {
                println!("❌ Height:     Rust={} vs Go=0", rust_qbft_msg.height);
            } else {
                println!("✓ Height:     0");
            }

            // Round comparison
            let go_round = go_state.round.map(|r| u64::from(r)).unwrap_or(0);
            let rust_round = rust_qbft_msg.round;
            if rust_round != go_round {
                println!("❌ Round:      Rust={} vs Go={}", rust_round, go_round);
            } else {
                println!("✓ Round:      {}", rust_round);
            }

            // Data Round
            let expected_data_round = if matches!(
                rust_qbft_msg.qbft_message_type,
                ssv_types::consensus::QbftMessageType::RoundChange
            ) {
                0
            } else {
                0
            };
            if rust_qbft_msg.data_round != expected_data_round {
                println!(
                    "❌ DataRound:  Rust={} vs Go={}",
                    rust_qbft_msg.data_round, expected_data_round
                );
            } else {
                println!("✓ DataRound:  {}", rust_qbft_msg.data_round);
            }

            // Root comparison
            if rust_qbft_msg.root != go_state.root {
                println!(
                    "❌ Root:       Rust={} vs Go={}",
                    hex::encode(rust_qbft_msg.root),
                    hex::encode(go_state.root)
                );
            } else {
                println!("✓ Root:       {}", hex::encode(rust_qbft_msg.root));
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
                println!(
                    "❌ RoundChangeJustifications: Rust={} vs Go={}",
                    rust_qbft_msg.round_change_justification.len(),
                    go_rc_count
                );
            } else {
                println!("✓ RoundChangeJustifications: {}", go_rc_count);
            }

            // Detailed RoundChangeJustifications comparison
            if go_rc_count > 0 {
                println!("\n--- RoundChangeJustifications Details ---");
                if let Some(go_rc_justifications) = &go_state.round_change_justifications {
                    for (i, (rust_rc_bytes, go_rc)) in rust_qbft_msg
                        .round_change_justification
                        .iter()
                        .zip(go_rc_justifications.iter())
                        .enumerate()
                    {
                        println!("RoundChangeJustification #{}", i);
                        // Decode the rust bytes back to SignedSSVMessage
                        if let Ok(rust_rc) = SignedSSVMessage::from_ssz_bytes(rust_rc_bytes) {
                            self.compare_signed_messages(&rust_rc, go_rc, i);
                        } else {
                            println!("  ❌ Failed to decode Rust RoundChangeJustification #{}", i);
                        }
                    }
                }
            }

            if rust_qbft_msg.prepare_justification.len() != go_prep_count {
                println!(
                    "❌ PrepareJustifications: Rust={} vs Go={}",
                    rust_qbft_msg.prepare_justification.len(),
                    go_prep_count
                );
            } else {
                println!("✓ PrepareJustifications: {}", go_prep_count);
            }

            // Detailed PrepareJustifications comparison
            if go_prep_count > 0 {
                println!("\n--- PrepareJustifications Details ---");
                if let Some(go_prep_justifications) = &go_state.prepare_justifications {
                    for (i, (rust_prep_bytes, go_prep)) in rust_qbft_msg
                        .prepare_justification
                        .iter()
                        .zip(go_prep_justifications.iter())
                        .enumerate()
                    {
                        println!("PrepareJustification #{}", i);
                        // Decode the rust bytes back to SignedSSVMessage
                        if let Ok(rust_prep) = SignedSSVMessage::from_ssz_bytes(rust_prep_bytes) {
                            self.compare_signed_messages(&rust_prep, go_prep, i);
                        } else {
                            println!("  ❌ Failed to decode Rust PrepareJustification #{}", i);
                        }
                    }
                }
            }

            // Final hash comparison
            println!("\n=== FINAL HASH COMPARISON ===");
            let rust_hash = hex::encode(rust_msg.tree_hash_root());
            let expected_hash = hex::encode(self.expected_root);
            println!("Rust computed hash: {}", rust_hash);
            println!("Expected hash:      {}", expected_hash);
            if rust_hash != expected_hash {
                println!("❌ HASH MISMATCH - This is the root cause of test failure");
            } else {
                println!("✓ HASH MATCHES");
            }
        } else {
            println!("❌ Failed to decode Rust QBFT message from SSVMessage data");
        }
    }

    fn compare_signed_messages(
        &self,
        rust_msg: &SignedSSVMessage,
        go_msg: &SignedSSVMessage,
        index: usize,
    ) {
        println!("\n--- SignedSSVMessage #{} Comparison ---", index);

        // SignedSSVMessage level
        if rust_msg.signatures().len() != go_msg.signatures().len() {
            println!(
                "  ❌ Signatures count: Rust={} vs Go={}",
                rust_msg.signatures().len(),
                go_msg.signatures().len()
            );
        } else {
            println!("  ✓ Signatures count: {}", rust_msg.signatures().len());
        }

        if rust_msg.operator_ids() != go_msg.operator_ids() {
            println!(
                "  ❌ OperatorIDs: Rust={:?} vs Go={:?}",
                rust_msg.operator_ids(),
                go_msg.operator_ids()
            );
        } else {
            println!("  ✓ OperatorIDs: {:?}", rust_msg.operator_ids());
        }

        if rust_msg.full_data() != go_msg.full_data() {
            println!(
                "  ❌ FullData length: Rust={} vs Go={}",
                rust_msg.full_data().len(),
                go_msg.full_data().len()
            );
        } else {
            println!("  ✓ FullData length: {}", rust_msg.full_data().len());
        }

        // SSVMessage level
        let rust_ssv = rust_msg.ssv_message();
        let go_ssv = go_msg.ssv_message();

        if rust_ssv.msg_type() != go_ssv.msg_type() {
            println!(
                "  ❌ SSVMessage.MsgType: Rust={:?} vs Go={:?}",
                rust_ssv.msg_type(),
                go_ssv.msg_type()
            );
        } else {
            println!("  ✓ SSVMessage.MsgType: {:?}", rust_ssv.msg_type());
        }

        if rust_ssv.msg_id() != go_ssv.msg_id() {
            println!(
                "  ❌ SSVMessage.MsgID: Rust={} vs Go={}",
                hex::encode(rust_ssv.msg_id()),
                hex::encode(go_ssv.msg_id())
            );
        } else {
            println!("  ✓ SSVMessage.MsgID: {}", hex::encode(rust_ssv.msg_id()));
        }

        if rust_ssv.data().len() != go_ssv.data().len() {
            println!(
                "  ❌ SSVMessage.Data length: Rust={} vs Go={}",
                rust_ssv.data().len(),
                go_ssv.data().len()
            );
        } else {
            println!("  ✓ SSVMessage.Data length: {}", rust_ssv.data().len());
        }

        if rust_ssv.data() != go_ssv.data() {
            println!("  ❌ SSVMessage.Data content differs:");
            println!("    Rust: {}", hex::encode(rust_ssv.data()));
            println!("    Go:   {}", hex::encode(go_ssv.data()));

            // Try to decode and compare QbftMessage if possible
            if let (Ok(rust_qbft), Ok(go_qbft)) = (
                ssv_types::consensus::QbftMessage::from_ssz_bytes(rust_ssv.data()),
                ssv_types::consensus::QbftMessage::from_ssz_bytes(go_ssv.data()),
            ) {
                println!("  --- QbftMessage comparison ---");
                if rust_qbft.qbft_message_type != go_qbft.qbft_message_type {
                    println!(
                        "    ❌ QbftMessage.Type: Rust={:?} vs Go={:?}",
                        rust_qbft.qbft_message_type, go_qbft.qbft_message_type
                    );
                }
                if rust_qbft.height != go_qbft.height {
                    println!(
                        "    ❌ QbftMessage.Height: Rust={} vs Go={}",
                        rust_qbft.height, go_qbft.height
                    );
                }
                if rust_qbft.round != go_qbft.round {
                    println!(
                        "    ❌ QbftMessage.Round: Rust={} vs Go={}",
                        rust_qbft.round, go_qbft.round
                    );
                }
                if rust_qbft.root != go_qbft.root {
                    println!(
                        "    ❌ QbftMessage.Root: Rust={} vs Go={}",
                        hex::encode(rust_qbft.root),
                        hex::encode(go_qbft.root)
                    );
                }
            }
        } else {
            println!("  ✓ SSVMessage.Data content matches");
        }
    }
}
