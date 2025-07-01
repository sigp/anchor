#![allow(dead_code)]

mod constants;
mod qbft;
mod ssv;
mod types;
mod utils;
use std::{
    collections::{HashMap, HashSet},
    fmt, fs,
    path::Path,
    sync::LazyLock,
};

use qbft::QbftSpecTestType;
use serde::de::DeserializeOwned;
use ssv::SsvSpecTestType;
use types::TypesSpecTestType;
use walkdir::WalkDir;

use crate::qbft::*;
use crate::ssv::*;
use crate::types::*;

// All Spec Test Variants. Maps to an inner variant type that describes specific tests
#[derive(Eq, PartialEq, Hash, Debug)]
enum SpecTestType {
    Qbft(QbftSpecTestType),
    Ssv(SsvSpecTestType),
    Types(TypesSpecTestType),
}

// Maps a test category to its respective spec test location. Do not change!
impl fmt::Display for SpecTestType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            SpecTestType::Qbft(_) => write!(f, "src/ssv-spec/qbft/spectest/generate/tests"),
            SpecTestType::Ssv(_) => write!(f, "src/ssv-spec/ssv/spectest/generate/tests"),
            SpecTestType::Types(_) => write!(f, "src/ssv-spec/types/spectest/generate/tests"),
        }
    }
}

impl SpecTestType {
    /// Some tests are encoding tests. They share a prefix but have a different fielname and
    /// structure
    pub fn is_encoding(&self) -> bool {
        if let SpecTestType::Types(type_test) = self {
            type_test.is_encoding()
        } else {
            false
        }
    }
}

// Import the debug_encoding module

// Core trait to orchestrate setting up and running spec tests. The spec tests are broken up into
// different categories with different file strucutres. For each file structure, implementing the
// required functions allows for a smooth testing process
trait SpecTest {
    // Retrieve the name of the test
    fn name(&self) -> &str;

    // Setup a runner for the test. This will configure and construct eveything required to
    // execute the test
    fn setup(&mut self);

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
    // Qbft tests
    // ----------
    TimeoutTest,
    CreateMessageTest,
    // SSV tests
    // ---------
    CommitteeSpecTest,
    MultiCommitteeSpecTest,
    MultiStartNewRunnerDutySpecTest,
    PartialSigContainerTest,
    RunnerConstructionSpecTest,
    SyncCommitteeAggregatorProofSpecTest,
    MsgProcessingSpecTest,
    MultiMsgProcessingSpecTest,
    ValCheckSpecTest,
    MultiValCheckSpecTest,
    // Types tests
    // -----------
    BeaconDepositDataTest,
    BeaconVoteEncodingTest,
    CommitteeMemberTest,
    DutySpecTest,
    EncryptionSpecTest,
    MaxMsgSizeTest,
    PartialSigMsgSpecTest,
    PartialSigMessageEncodingTest,
    ShareEncodingTest,
    SignedSSVMessageTest,
    SignedSSVMessageEncodingTest,
    SSVMessageTest,
    SSVMessageEncodingTest,
    SSZSpecTest,
    ValidatorConsensusDataTest,
    ValidatorConsensusDataEncodingTest,
);

// Register a test in the loader. This inserts a mapping from SpecTestType -> loading closure
// into a map for later access. This is needed to that we can parse from an arbitrary test file to a
// specific test type T
fn register_test<T: SpecTest + DeserializeOwned + 'static>(map: &mut Loaders) {
    map.insert(T::test_type(), |path| {
        let contents = fs::read_to_string(path)
            .unwrap_or_else(|_| panic!("Failed to read test file: {}", path));

        let test: T = serde_json::from_str(&contents).unwrap_or_else(|e| {
            eprintln!("=== JSON PARSING ERROR ===");
            eprintln!("File: {}", path);
            eprintln!("Error: {}", e);
            eprintln!("========================");
            panic!("Failed to parse test {}: {}", path, e)
        });

        Box::new(test)
    });
}

// Core function to run the tests. Given a SpecTestType, it will navigate to the proper directory,
// read in all of the tests, make sure they are all setup, and then run each one
fn run_tests(test_type: SpecTestType) -> bool {
    let dir_name = test_type.to_string();
    let test_dir = Path::new(&dir_name);

    let tests: Vec<Box<dyn SpecTest>> = WalkDir::new(test_dir)
        .into_iter()
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();

            // Check if it is an encoding test
            let is_encoding = test_type.is_encoding();

            // Get the inner variant string to check in filenames
            let variant = match &test_type {
                SpecTestType::Qbft(inner) => inner.to_string(),
                SpecTestType::Ssv(inner) => inner.to_string(),
                SpecTestType::Types(inner) => inner.to_string(),
            };

            if path.is_file() {
                let filename = path.file_name().map(|name| name.to_string_lossy());

                let matches = filename
                    .map(|name| {
                        let split: HashSet<String> = name.split('.').map(String::from).collect();

                        let contains_prefix = split.contains(&variant);
                        if is_encoding {
                            // if it is an encoding tests, we also have to check that the file
                            // conatins "EncodingTest"
                            contains_prefix & name.contains("EncodingTest")
                        } else {
                            contains_prefix & !name.contains("EncodingTest")
                        }
                    })
                    .unwrap_or(false);

                if matches {
                    println!("Loading {:?}", path);
                    let loader = TEST_LOADERS
                        .get(&test_type)
                        .unwrap_or_else(|| panic!("No loader registered for:{}", test_type));
                    return Some(loader(&path.to_string_lossy()));
                }
            }
            None
        })
        .collect();

    assert!(tests.len() != 0);
    println!("Loaded {} tests", tests.len());

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

    // All Qbft specific spec tests
    mod qbft_tests {
        use super::*;

        #[test]
        fn test_qbft_timeout() {
            assert!(run_tests(SpecTestType::Qbft(QbftSpecTestType::Timeout)))
        }

        #[test]
        fn test_qbft_create() {
            assert!(run_tests(SpecTestType::Qbft(
                QbftSpecTestType::CreateMessage
            )))
        }
    }

    // All SSV specific spec tests
    mod ssv_tests {
        use super::*;

        #[test]
        fn test_ssv_committee() {
            assert!(run_tests(SpecTestType::Ssv(SsvSpecTestType::Committee)))
        }

        #[test]
        fn test_ssv_multi_committee() {
            assert!(run_tests(SpecTestType::Ssv(
                SsvSpecTestType::MultiCommittee
            )))
        }

        #[test]
        fn test_ssv_new_duty() {
            assert!(run_tests(SpecTestType::Ssv(SsvSpecTestType::NewDuty)))
        }

        #[test]
        fn test_ssv_partial_sig_container() {
            assert!(run_tests(SpecTestType::Ssv(
                SsvSpecTestType::PartialSigContainer
            )))
        }

        #[test]
        fn test_ssv_runner_construction() {
            assert!(run_tests(SpecTestType::Ssv(
                SsvSpecTestType::RunnerConstruction
            )))
        }

        #[test]
        fn test_ssv_sync_committee_aggregator() {
            assert!(run_tests(SpecTestType::Ssv(
                SsvSpecTestType::SyncCommitteeAggregator
            )))
        }

        #[test]
        fn test_ssv_msg_processing() {
            assert!(run_tests(SpecTestType::Ssv(SsvSpecTestType::MsgProcessing)))
        }

        #[test]
        fn test_ssv_multi_msg_processing() {
            assert!(run_tests(SpecTestType::Ssv(
                SsvSpecTestType::MultiMsgProcessing
            )))
        }

        #[test]
        fn test_ssv_val_check() {
            assert!(run_tests(SpecTestType::Ssv(SsvSpecTestType::ValCheck)))
        }

        #[test]
        fn test_ssv_multi_val_check() {
            assert!(run_tests(SpecTestType::Ssv(SsvSpecTestType::MultiValCheck)))
        }
    }

    // All type specific spec tests
    mod type_tests {
        use super::*;

        #[test]
        // Beacon vote deposit data
        fn test_types_beacon_deposit_data_test() {
            assert!(run_tests(SpecTestType::Types(
                TypesSpecTestType::BeaconDepositData
            )))
        }

        #[test]
        // Beacon vote encoding
        fn test_types_beacon_vote_encoding_test() {
            assert!(run_tests(SpecTestType::Types(
                TypesSpecTestType::BeaconVoteEncoding
            )))
        }

        #[test]
        // Committee member test
        fn test_types_committee_member_test() {
            assert!(run_tests(SpecTestType::Types(
                TypesSpecTestType::CommitteeMember
            )))
        }

        #[test]
        // Duty spec tests
        fn test_types_duty_test() {
            assert!(run_tests(SpecTestType::Types(TypesSpecTestType::Duty)))
        }

        #[test]
        // Encryption test
        fn test_types_encryption_test() {
            assert!(run_tests(SpecTestType::Types(
                TypesSpecTestType::Encryption
            )))
        }

        #[test]
        // Max msg size tests
        fn test_types_max_message_size() {
            assert!(run_tests(SpecTestType::Types(
                TypesSpecTestType::MaxMsgSize
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
        fn test_types_partial_sig_message_encoding() {
            assert!(run_tests(SpecTestType::Types(
                TypesSpecTestType::PartialSigMessageEncoding
            )))
        }

        #[test]
        // Share encoding encoding
        fn test_types_share_encoding() {
            assert!(run_tests(SpecTestType::Types(
                TypesSpecTestType::ShareEncoding
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
        fn test_types_signed_ssv_message_encoding() {
            assert!(run_tests(SpecTestType::Types(
                TypesSpecTestType::SignedSSVMsgEncoding
            )))
        }

        #[test]
        // Signed SSV Message Encoding
        fn test_types_ssv_message_encoding() {
            assert!(run_tests(SpecTestType::Types(
                TypesSpecTestType::SSVMsgEncoding
            )))
        }

        #[test]
        // SSZ withdrawals marshalling test
        fn test_types_ssz() {
            assert!(run_tests(SpecTestType::Types(TypesSpecTestType::SSZ)))
        }

        #[test]
        // Validator consensus data encoding
        fn test_types_validator_consensus_data() {
            assert!(run_tests(SpecTestType::Types(
                TypesSpecTestType::ValidatorConsensusData
            )))
        }

        #[test]
        // Validator consensus data encoding
        fn test_types_validator_consensus_data_encoding() {
            assert!(run_tests(SpecTestType::Types(
                TypesSpecTestType::ValidatorConsensusDataEncoding
            )))
        }
    }
}
