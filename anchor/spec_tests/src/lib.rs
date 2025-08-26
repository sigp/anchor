#![allow(dead_code)]
#![recursion_limit = "512"]

pub mod qbft;
mod types;
mod utils;

use qbft::QbftSpecTestType;
use serde::de::DeserializeOwned;
use std::{
    collections::{HashMap, HashSet},
    fmt, fs,
    path::Path,
    sync::LazyLock,
};
use types::TypesSpecTestType;
use walkdir::WalkDir;

use crate::qbft::*;
use crate::types::*;

// All Spec Test Variants. Maps to an inner variant type that describes specific tests
#[derive(Eq, PartialEq, Hash, Debug)]
enum SpecTestType {
    Types(TypesSpecTestType),
    Qbft(QbftSpecTestType),
}

// Maps a test category to its respective spec test location. Do not change!
impl fmt::Display for SpecTestType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            SpecTestType::Types(_) => write!(f, "ssv-spec/types/spectest/generate/tests"),
            SpecTestType::Qbft(_) => write!(f, "ssv-spec/qbft/spectest/generate/tests"),
        }
    }
}

impl SpecTestType {
    /// Some tests are encoding tests. They share a prefix but have a different fielname and
    /// structure
    pub fn is_encoding(&self) -> bool {
        matches!(self, SpecTestType::Types(type_test) if type_test.is_encoding())
    }
}

// Import the debug_encoding module

// Core trait to orchestrate setting up and running spec tests. The spec tests are broken up into
// different categories with different file strucutres. For each file structure, implementing the
// required functions allows for a smooth testing process
trait SpecTest {
    // Retrieve the name of the test
    fn name(&self) -> &str {
        ""
    }

    // Setup a runner for the test. This will configure and construct eveything required to
    // execute the test. Default implementation does nothing.
    fn setup(&mut self) {}

    // Run the test and verify that the output is what we were expecting.
    fn run(&self) -> bool;

    // Return the type of this test. Used as a Key for the loaders and path construction
    fn test_type() -> SpecTestType
    where
        Self: Sized;
}

// Abstract away repeated logic for registering a test type with the loader
macro_rules! register_test_loaders {
    ($($test_type:ty),* $(,)?) => {
        LazyLock::new(|| {
            let mut loaders = HashMap::new();
            $(
                register_test::<$test_type>(&mut loaders);
            )*
            loaders
        })
    };
}

type Loaders = HashMap<SpecTestType, fn(&str) -> Box<dyn SpecTest>>;
static TEST_LOADERS: LazyLock<Loaders> = register_test_loaders!(
    // Types tests
    // -----------
    BeaconVoteEncodingTest,
    ConsensusDataProposerTest,
    EncryptionSpecTest,
    PartialSigMsgSpecTest,
    PartialSigMessageEncodingTest,
    SignedSSVMessageTest,
    SignedSSVMessageEncodingTest,
    SSVMessageTest,
    SSVMessageEncodingTest,
    SSZSpecTest,
    ValidatorConsensusDataTest,
    ValidatorConsensusDataEncodingTest,
    // Qbft tests
    // ----------
    CreateMessageTest,
    MessageProcessingTest,
    QbftMessageTest,
    RoundRobinTest,
    TimeoutTest,
);

// Register a test in the loader. This inserts a mapping from SpecTestType -> loading closure
// into a map for later access. This is needed to that we can parse from an arbitrary test file to a
// specific test type T
fn register_test<T: SpecTest + DeserializeOwned + 'static>(map: &mut Loaders) {
    map.insert(T::test_type(), |path| {
        let contents =
            fs::read_to_string(path).unwrap_or_else(|_| panic!("Failed to read test file: {path}"));

        let test: T = serde_json::from_str(&contents).unwrap_or_else(|e| {
            eprintln!("=== JSON PARSING ERROR ===");
            eprintln!("File: {path}");
            eprintln!("Error: {e}");
            eprintln!("========================");
            panic!("Failed to parse test {path}: {e}")
        });

        Box::new(test)
    });
}

// Core function to run the tests. Given a SpecTestType, it will navigate to the proper directory,
// read in all of the tests, make sure they are all setup, and then run each one
fn run_tests(test_type: SpecTestType) -> bool {
    let dir_name = test_type.to_string();
    let test_dir = Path::new(&dir_name);

    let mut tests: Vec<Box<dyn SpecTest>> = WalkDir::new(test_dir)
        .into_iter()
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();

            // Check if it is an encoding test
            let is_encoding = test_type.is_encoding();

            // Get the inner variant string to check in filenames
            let variant = match &test_type {
                SpecTestType::Types(inner) => inner.to_string(),
                SpecTestType::Qbft(inner) => inner.to_string(),
            };

            if path.is_file() {
                let filename = path.file_name().map(|name| name.to_string_lossy());
                let matches = filename
                    .as_ref()
                    .map(|name| {
                        let split: HashSet<String> = name.split('.').map(String::from).collect();

                        // Check if any chunk contains the variant as a prefix to avoid false
                        // matches (e.g., "ssvmsg" matching "signedssvmsg")
                        let contains_prefix = split
                            .iter()
                            .any(|chunk| chunk.starts_with(&variant) || chunk == &variant);

                        if is_encoding {
                            // if it is an encoding tests, we also have to check that the file
                            // conatins "EncodingTest"
                            contains_prefix & name.contains("EncodingTest")
                        } else {
                            // Special case: For MsgSpecTest, exclude CreateMsgSpecTest
                            let exclude_create = if variant == "MsgSpecTest" {
                                !name.contains("CreateMsgSpecTest")
                            } else {
                                true
                            };
                            contains_prefix & !name.contains("EncodingTest") & exclude_create
                        }
                    })
                    .unwrap_or(false);

                if matches {
                    let loader = TEST_LOADERS
                        .get(&test_type)
                        .unwrap_or_else(|| panic!("No loader registered for: {test_type}"));
                    return Some(loader(&path.to_string_lossy()));
                }
            }
            None
        })
        .collect();

    let mut result = true;
    for mut test in tests {
        test.setup();
        result &= test.run();
    }
    result
}

#[cfg(test)]
mod spec_tests {
    use super::*;

    // All qbft specific tests
    mod qbft_tests {
        use super::*;

        #[test]
        fn test_qbft_create() {
            assert!(run_tests(SpecTestType::Qbft(
                QbftSpecTestType::CreateMessage
            )))
        }

        #[test]
        fn test_qbft_timeout() {
            assert!(run_tests(SpecTestType::Qbft(QbftSpecTestType::Timeout)))
        }

        #[test]
        fn test_qbft_message() {
            assert!(run_tests(SpecTestType::Qbft(QbftSpecTestType::QbftMessage)))
        }

        #[test]
        fn test_qbft_processing() {
            assert!(run_tests(SpecTestType::Qbft(
                QbftSpecTestType::MsgProcessing
            )))
        }

        #[test]
        fn test_qbft_round_robin() {
            assert!(run_tests(SpecTestType::Qbft(QbftSpecTestType::RoundRobin)))
        }
    }

    // All type specific spec tests
    mod type_tests {
        use super::*;

        #[test]
        // Beacon vote encoding
        fn test_types_encoding_beacon_vote() {
            assert!(run_tests(SpecTestType::Types(
                TypesSpecTestType::BeaconVoteEncoding
            )))
        }

        #[test]
        // Consensus data proposer test
        #[ignore = "invalid signature and block encoding"]
        fn test_types_consensus_data_proposer() {
            assert!(run_tests(SpecTestType::Types(
                TypesSpecTestType::ConsensusDataProposer
            )))
        }

        #[test]
        // Encryption test
        fn test_types_encryption_test() {
            assert!(run_tests(SpecTestType::Types(
                TypesSpecTestType::Encryption
            )))
        }

        #[test]
        // Partial sig message encoding
        fn test_types_partial_sig_message() {
            assert!(run_tests(SpecTestType::Types(
                TypesSpecTestType::PartialSigMessage
            )))
        }

        #[test]
        // Partial sig message encoding
        fn test_types_encoding_partial_sig_message() {
            assert!(run_tests(SpecTestType::Types(
                TypesSpecTestType::PartialSigMessageEncoding
            )))
        }

        #[test]
        // Signed ssv message test
        fn test_types_signed_ssv_message() {
            assert!(run_tests(SpecTestType::Types(
                TypesSpecTestType::SignedSSVMsg
            )))
        }

        #[test]
        // Signed SSV Message Encoding
        fn test_types_encoding_signed_ssv_message() {
            assert!(run_tests(SpecTestType::Types(
                TypesSpecTestType::SignedSSVMsgEncoding
            )))
        }

        #[test]
        // SSV Message test
        fn test_types_ssv_message() {
            assert!(run_tests(SpecTestType::Types(TypesSpecTestType::SSVMsg)))
        }

        #[test]
        // Signed SSV Message Encoding
        fn test_types_encoding_ssv_message() {
            assert!(run_tests(SpecTestType::Types(
                TypesSpecTestType::SSVMsgEncoding
            )))
        }

        #[test]
        #[ignore = "invalid signature in test data"]
        // SSZ withdrawals marshalling test
        fn test_types_ssz() {
            assert!(run_tests(SpecTestType::Types(TypesSpecTestType::Ssz)))
        }

        #[test]
        #[ignore = "need to implement validation"]
        // Validator consensus data encoding
        fn test_types_validator_consensus_data() {
            assert!(run_tests(SpecTestType::Types(
                TypesSpecTestType::ValidatorConsensusData
            )))
        }

        #[test]
        // Validator consensus data encoding
        fn test_types_encoding_validator_consensus_data() {
            assert!(run_tests(SpecTestType::Types(
                TypesSpecTestType::ValidatorConsensusDataEncoding
            )))
        }
    }
}
