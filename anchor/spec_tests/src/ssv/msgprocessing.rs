use serde::Deserialize;
use ssv_types::{
    CommitteeId, OperatorId, ValidatorIndex, consensus::BeaconRole, domain_type::DomainType,
    message::SSVMessage,
};
use std::collections::HashMap;
use types::{Address, CommitteeIndex, Graffiti, Hash256, PublicKeyBytes, Slot};

use crate::{SpecTest, SpecTestType, ssv::SsvSpecTestType, ssv::ssv_deserializers::*};

// Message structure with signatures and operator IDs
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TestMessage {
    #[serde(rename = "Signatures")]
    pub signatures: Vec<String>, // Base64 encoded signatures

    #[serde(rename = "OperatorIDs")]
    #[serde(deserialize_with = "deserialize_operator_ids")]
    pub operator_ids: Vec<OperatorId>,

    #[serde(rename = "SSVMessage")]
    pub ssv_message: SSVMessage, // Strongly typed SSV message

    #[serde(rename = "FullData")]
    pub full_data: Option<String>, // Optional full data (base64 encoded)
}

// Helper deserializer for operator IDs in messages
fn deserialize_operator_ids<'de, D>(deserializer: D) -> Result<Vec<OperatorId>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let ids = Vec::<u64>::deserialize(deserializer)?;
    Ok(ids.into_iter().map(OperatorId::from).collect())
}

// Test-specific ValidatorDuty with proper field renames
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TestValidatorDuty {
    #[serde(rename = "Type")]
    pub duty_type: BeaconRole,

    #[serde(
        rename = "PubKey",
        deserialize_with = "deserialize_public_key_bytes_from_hex"
    )]
    pub pub_key: PublicKeyBytes,

    #[serde(rename = "Slot", deserialize_with = "deserialize_slot_from_string")]
    pub slot: Slot,

    #[serde(
        rename = "ValidatorIndex",
        deserialize_with = "deserialize_validator_index_from_string"
    )]
    pub validator_index: ValidatorIndex,

    #[serde(rename = "CommitteeIndex")]
    pub committee_index: CommitteeIndex,

    #[serde(rename = "CommitteeLength")]
    pub committee_length: u64,

    #[serde(rename = "CommitteesAtSlot")]
    pub committees_at_slot: u64,

    #[serde(rename = "ValidatorCommitteeIndex")]
    pub validator_committee_index: u64,

    #[serde(rename = "ValidatorSyncCommitteeIndices")]
    pub validator_sync_committee_indices: Option<Vec<u64>>, // Simplified as Vec instead of VariableList
}

// Committee duty structure
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommitteeDuty {
    #[serde(rename = "Slot", deserialize_with = "deserialize_slot_from_string")]
    pub slot: Slot,

    #[serde(rename = "ValidatorDuties")]
    pub validator_duties: Vec<TestValidatorDuty>,
}

// QBFT Controller committee member
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QBFTCommitteeMember {
    #[serde(rename = "OperatorID", deserialize_with = "deserialize_operator_id")]
    pub operator_id: OperatorId,

    #[serde(rename = "CommitteeID", deserialize_with = "deserialize_committee_id")]
    pub committee_id: CommitteeId,

    #[serde(
        rename = "SSVOperatorPubKey",
        deserialize_with = "deserialize_rsa_public_key"
    )]
    pub ssv_operator_pub_key: Vec<u8>,

    #[serde(rename = "FaultyNodes")]
    pub faulty_nodes: u64,

    #[serde(rename = "Committee")]
    pub committee: Vec<TestQBFTOperator>, // QBFT operators

    #[serde(rename = "DomainType", deserialize_with = "deserialize_domain_type")]
    pub domain_type: DomainType,
}

// QBFT Committee operator
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TestQBFTOperator {
    #[serde(rename = "OperatorID", deserialize_with = "deserialize_operator_id")]
    pub operator_id: OperatorId,

    #[serde(
        rename = "SSVOperatorPubKey",
        deserialize_with = "deserialize_rsa_public_key"
    )]
    pub ssv_operator_pub_key: Vec<u8>,
}

// QBFT Controller structure
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QBFTController {
    #[serde(rename = "Identifier")]
    pub identifier: String, // Base64 encoded identifier

    #[serde(rename = "Height")]
    pub height: u64,

    #[serde(rename = "StoredInstances")]
    pub stored_instances: Vec<String>, // Array of stored instances (can be empty)

    #[serde(rename = "CommitteeMember")]
    pub committee_member: QBFTCommitteeMember,
}

// Test-specific share committee member structure
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TestShareCommitteeMember {
    #[serde(rename = "SharePubKey")]
    pub share_pub_key: String, // Base64 encoded

    #[serde(rename = "Signer")]
    pub signer: u64,
}

// Test-specific share structure (different from main ssv_types::Share)
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TestShare {
    #[serde(
        rename = "ValidatorIndex",
        deserialize_with = "deserialize_validator_index_from_string"
    )]
    pub validator_index: ValidatorIndex,

    #[serde(
        rename = "ValidatorPubKey",
        deserialize_with = "deserialize_public_key_bytes"
    )]
    pub validator_pub_key: PublicKeyBytes,

    #[serde(rename = "SharePubKey")]
    pub share_pub_key: String, // Base64 encoded

    #[serde(rename = "Committee")]
    pub committee: Vec<TestShareCommitteeMember>,

    #[serde(rename = "DomainType", deserialize_with = "deserialize_domain_type")]
    pub domain_type: DomainType,

    #[serde(
        rename = "FeeRecipientAddress",
        deserialize_with = "deserialize_address"
    )]
    pub fee_recipient_address: Address,

    #[serde(rename = "Graffiti", deserialize_with = "deserialize_graffiti")]
    pub graffiti: Graffiti,
}

// Base runner structure
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BaseRunner {
    #[serde(rename = "State")]
    pub state: Option<String>, // State can be null or hex string

    #[serde(rename = "Share")]
    pub share: HashMap<String, TestShare>, // Map of validator index -> share

    #[serde(rename = "QBFTController")]
    pub qbft_controller: QBFTController, // QBFT controller

    #[serde(rename = "BeaconNetwork")]
    pub beacon_network: String, // Network identifier

    #[serde(rename = "RunnerRoleType")]
    pub runner_role_type: u64, // Role type number
}

// Runner structure containing BaseRunner
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TestRunner {
    #[serde(rename = "BaseRunner")]
    pub base_runner: BaseRunner,
}

// Message processing test for single messages
#[derive(Debug, Deserialize)]
pub struct MsgProcessingSpecTest {
    #[serde(rename = "Name")]
    pub name: String,

    #[serde(rename = "Runner")]
    pub runner: TestRunner, // Strongly typed runner structure

    #[serde(rename = "Messages")]
    pub messages: Vec<TestMessage>, // Array of messages to process

    #[serde(rename = "BeaconBroadcastedRoots")]
    #[serde(deserialize_with = "deserialize_optional_hash_vec", default)]
    pub beacon_broadcasted_roots: Option<Vec<Hash256>>,

    #[serde(rename = "DontStartDuty")]
    pub dont_start_duty: bool,

    #[serde(rename = "ExpectedError")]
    pub expected_error: String,

    #[serde(rename = "CommitteeDuty", default)]
    pub committee_duty: Option<CommitteeDuty>,

    #[serde(rename = "DecidedSlashable", default)]
    pub decided_slashable: Option<bool>, // Optional slashable decision

    #[serde(rename = "PostDutyRunnerStateRoot", default)]
    pub post_duty_runner_state_root: Option<String>, // Optional post-duty state root

    // Catch any other unknown fields for now
    #[serde(flatten)]
    pub additional_fields: HashMap<String, serde_json::Value>,
}

impl SpecTest for MsgProcessingSpecTest {
    fn name(&self) -> &str {
        &self.name
    }

    fn setup(&mut self) {
        // Setup any required test state
    }

    fn run(&self) -> bool {
        println!("Running message processing test: {}", self.name);

        // Validate runner structure
        let base_runner = &self.runner.base_runner;
        println!("Runner state: {:?}", base_runner.state);
        println!("Share count: {}", base_runner.share.len());

        // Validate each share
        for (validator_idx, share) in &base_runner.share {
            println!(
                "Share for validator {}: index {}",
                validator_idx, share.validator_index.0
            );
            println!("  Validator pub key: {:?}", share.validator_pub_key);
            println!("  Committee size: {}", share.committee.len());
            println!("  Domain type: {:?}", share.domain_type);
            println!("  Fee recipient: {:?}", share.fee_recipient_address);

            // Validate committee members
            for (i, member) in share.committee.iter().enumerate() {
                println!("    Committee member {}: signer {}", i + 1, member.signer);
            }
        }

        true
    }

    fn test_type() -> SpecTestType {
        SpecTestType::Ssv(SsvSpecTestType::MsgProcessing)
    }
}

// Individual test case within a multi-message processing test
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MultiMsgProcessingTestCase {
    #[serde(rename = "Name")]
    pub name: String,

    #[serde(rename = "Runner")]
    pub runner: TestRunner, // Strongly typed runner structure
}

// Multi-message processing test
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MultiMsgProcessingSpecTest {
    #[serde(rename = "Name")]
    pub name: String,

    #[serde(rename = "Tests")]
    pub tests: Vec<MultiMsgProcessingTestCase>,
}

impl SpecTest for MultiMsgProcessingSpecTest {
    fn name(&self) -> &str {
        &self.name
    }

    fn setup(&mut self) {
        // Setup any required test state
    }

    fn run(&self) -> bool {
        println!("Running multi-message processing test: {}", self.name);
        println!("Test cases: {}", self.tests.len());

        for (i, test_case) in self.tests.iter().enumerate() {
            println!("  Test case {}: {}", i + 1, test_case.name);
        }

        // TODO: Implement multi-message processing test logic for each test case
        true
    }

    fn test_type() -> SpecTestType {
        SpecTestType::Ssv(SsvSpecTestType::MultiMsgProcessing)
    }
}
