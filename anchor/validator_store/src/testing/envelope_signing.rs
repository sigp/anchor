//! Envelope-signing duty tests (SIP-94).

use bls::{FixedBytesExtended, PublicKeyBytes};
use qbft::Completed;
use qbft_manager::{ConsensusDecider, EnvelopeProposerInstanceId, TimeoutMode};
use ssv_types::{
    IndexSet, OperatorId, ValidatorIndex,
    consensus::{
        BEACON_ROLE_ENVELOPE_PROPOSER, BlindedExecutionPayloadEnvelope, DataVersion,
        EnvelopeConsensusData, EnvelopeConsensusDataValidator, QbftDataValidator, ValidatorDuty,
    },
};
use ssz::Encode;
use ssz_types::VariableList;
use tokio::time::Instant;
use types::{
    ExecutionPayloadEnvelope, ExecutionPayloadGloas, ExecutionRequestsGloas, ForkName, Hash256,
    MainnetEthSpec, Slot, consts::gloas::BUILDER_INDEX_SELF_BUILD,
};

use super::common::*;

/// Validator index the single-validator committee starts at.
const STARTING_VALIDATOR_INDEX: usize = 5;

fn test_operator_ids() -> [OperatorId; 4] {
    [OperatorId(1), OperatorId(2), OperatorId(3), OperatorId(4)]
}

/// Builds a Gloas self-build envelope at `TEST_SLOT` bound to `beacon_block_root`.
fn self_build_envelope(beacon_block_root: Hash256) -> ExecutionPayloadEnvelope<MainnetEthSpec> {
    ExecutionPayloadEnvelope {
        payload: ExecutionPayloadGloas::<MainnetEthSpec> {
            slot_number: Slot::new(TEST_SLOT),
            ..Default::default()
        },
        execution_requests: ExecutionRequestsGloas::<MainnetEthSpec>::default(),
        builder_index: BUILDER_INDEX_SELF_BUILD,
        beacon_block_root,
        parent_beacon_block_root: Hash256::from_low_u64_be(0x1111),
    }
}

/// Wraps a full envelope into the consensus value the store proposes for it.
fn envelope_consensus_value(
    pubkey: PublicKeyBytes,
    envelope: &ExecutionPayloadEnvelope<MainnetEthSpec>,
) -> EnvelopeConsensusData {
    let blinded = BlindedExecutionPayloadEnvelope::from_full(envelope);
    EnvelopeConsensusData {
        duty: ValidatorDuty {
            r#type: BEACON_ROLE_ENVELOPE_PROPOSER,
            pub_key: pubkey,
            slot: Slot::new(TEST_SLOT),
            validator_index: ValidatorIndex(STARTING_VALIDATOR_INDEX),
            committee_index: 0,
            committee_length: 0,
            committees_at_slot: 0,
            validator_committee_index: 0,
            validator_sync_committee_indices: Default::default(),
        },
        version: DataVersion::from(ForkName::Gloas),
        data_ssz: VariableList::new(blinded.as_ssz_bytes())
            .expect("blinded envelope bytes should fit in the consensus data list"),
    }
}

/// Drives one envelope decide call against the mock and unwraps the decided value.
async fn decide_envelope_seed(
    decider: &MockConsensusDecider,
    pubkey: PublicKeyBytes,
    seed: EnvelopeConsensusData,
) -> EnvelopeConsensusData {
    let result = ConsensusDecider::<MainnetEthSpec>::decide_instance(
        decider,
        EnvelopeProposerInstanceId {
            validator: pubkey,
            instance_height: (TEST_SLOT as usize).into(),
        },
        seed,
        Box::new(EnvelopeConsensusDataValidator::<MainnetEthSpec>::new(
            pubkey,
            ValidatorIndex(STARTING_VALIDATOR_INDEX),
            Slot::new(TEST_SLOT),
            Hash256::zero(),
        )),
        TimeoutMode::Relative {
            current_round_start_time: Instant::now(),
        },
        &IndexSet::from(test_operator_ids()),
    )
    .await
    .expect("the mock decider must not fail");
    match result {
        Completed::Success(decided) => decided,
        Completed::TimedOut => panic!("the mock decider must not time out"),
    }
}

/// A forced envelope decision replaces the echo for envelope seeds.
#[tokio::test(flavor = "multi_thread")]
async fn mock_deciding_envelope_returns_the_forced_value() {
    let pubkey = PublicKeyBytes::empty();
    let seed_envelope = self_build_envelope(Hash256::from_low_u64_be(0xaaaa));
    let mut forced_envelope = seed_envelope.clone();
    forced_envelope.parent_beacon_block_root = Hash256::from_low_u64_be(0xbbbb);
    let seed = envelope_consensus_value(pubkey, &seed_envelope);
    let forced = envelope_consensus_value(pubkey, &forced_envelope);

    let decider = MockConsensusDecider::deciding_envelope(forced.clone());
    let decided = decide_envelope_seed(&decider, pubkey, seed).await;

    assert_eq!(
        decided, forced,
        "an envelope seed must decide as the forced value, not the echo"
    );
}

/// Without a forced decision, an envelope seed echoes back unchanged.
#[tokio::test(flavor = "multi_thread")]
async fn mock_echoes_envelope_seed_without_a_forced_decision() {
    let pubkey = PublicKeyBytes::empty();
    let seed = envelope_consensus_value(
        pubkey,
        &self_build_envelope(Hash256::from_low_u64_be(0xaaaa)),
    );

    let decider = MockConsensusDecider::echoing();
    let decided = decide_envelope_seed(&decider, pubkey, seed.clone()).await;

    assert_eq!(
        decided, seed,
        "the echoing mock must return the seed unchanged"
    );
}

/// Every decide call lands in the capture, tagged with the seed type.
#[tokio::test(flavor = "multi_thread")]
async fn mock_captures_envelope_decide_calls() {
    let pubkey = PublicKeyBytes::empty();
    let seed = envelope_consensus_value(
        pubkey,
        &self_build_envelope(Hash256::from_low_u64_be(0xaaaa)),
    );

    let decider = MockConsensusDecider::echoing();
    let captured = decider.captured_decides();
    decide_envelope_seed(&decider, pubkey, seed).await;

    let calls = captured.lock();
    assert_eq!(calls.len(), 1, "one decide call must be captured");
    assert!(
        calls[0].data_type.contains("EnvelopeConsensusData"),
        "the captured call must be tagged with the envelope seed type"
    );
}

/// The factory passes pubkey, index, slot, and decided root into the value check.
#[tokio::test(flavor = "multi_thread")]
async fn factory_wires_duty_metadata_into_the_value_check() {
    let committee = create_committee_setup(&test_operator_ids(), 1, STARTING_VALIDATOR_INDEX);
    let pubkey = committee.validators[0].public_key;
    let harness = ValidatorStoreTestHarness::new_with_options(
        vec![committee],
        OperatorId(1),
        HarnessOptions {
            spec: gloas_at_genesis_spec(),
            disable_slashing_protection: true,
            ..Default::default()
        },
    );
    let envelope = self_build_envelope(Hash256::from_low_u64_be(0xdec1));
    let value = envelope_consensus_value(pubkey, &envelope);

    let validator = harness
        .validator_store
        .create_envelope_consensus_data_validator(
            pubkey,
            ValidatorIndex(STARTING_VALIDATOR_INDEX),
            Slot::new(TEST_SLOT),
            envelope.beacon_block_root,
        );

    assert!(
        validator.validate(&value, &value),
        "a matched envelope value must pass the factory-built value check"
    );

    let mut wrong_slot = value.clone();
    wrong_slot.duty.slot = Slot::new(TEST_SLOT + 1);
    assert!(
        !validator.validate(&wrong_slot, &value),
        "a value with the wrong slot must fail the factory-built value check"
    );
}
