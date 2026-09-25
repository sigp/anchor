//! SIP-94 proposer folding and local envelope publication regressions.

use std::time::Duration;

use bls::{FixedBytesExtended, PublicKeyBytes};
use eth2::types::FullBlockContents;
use slot_clock::SlotClock;
use ssv_types::{
    OperatorId,
    consensus::{
        BEACON_ROLE_PROPOSER, DataVersion, GloasProposalData, ProposerConsensusData, ValidatorDuty,
    },
    msgid::Role,
    partial_sig::PartialSignatureKind,
};
use ssz::Encode;
use ssz_types::VariableList;
use tree_hash::TreeHash;
use types::{
    BeaconBlock, BeaconBlockGloas, ChainSpec, Domain, EmptyBlock, EthSpec,
    ExecutionPayloadEnvelope, ExecutionPayloadGloas, ExecutionRequestsGloas, ForkName, Hash256,
    MainnetEthSpec, SignedRoot, SigningData, Slot, consts::gloas::BUILDER_INDEX_SELF_BUILD,
};
use validator_store::{UnsignedBlock, ValidatorStore};

use super::common::*;
use crate::{BlindedExecutionPayloadEnvelope, DecidedBlockContext, Error, SpecificError};

const EXTERNAL_BUILDER: u64 = 7;

fn committee() -> (CommitteeSetup, PublicKeyBytes) {
    let setup = create_primary_committee_setup(1);
    let pubkey = setup.validators[0].public_key;
    (setup, pubkey)
}

fn options() -> HarnessOptions {
    HarnessOptions {
        spec: gloas_at_genesis_spec(),
        ..Default::default()
    }
}

fn harness() -> (ValidatorStoreTestHarness, PublicKeyBytes) {
    let (setup, pubkey) = committee();
    (
        ValidatorStoreTestHarness::new_with_options(vec![setup], OperatorId(1), options()),
        pubkey,
    )
}

fn block(spec: &ChainSpec, builder_index: u64) -> BeaconBlock<MainnetEthSpec> {
    let mut block = BeaconBlockGloas::<MainnetEthSpec>::empty(spec);
    block.slot = Slot::new(TEST_SLOT);
    block
        .body
        .signed_execution_payload_bid
        .message
        .builder_index = builder_index;
    block
        .body
        .signed_execution_payload_bid
        .message
        .execution_requests_root =
        ExecutionRequestsGloas::<MainnetEthSpec>::default().tree_hash_root();
    BeaconBlock::Gloas(block)
}

fn envelope(block: &BeaconBlock<MainnetEthSpec>) -> ExecutionPayloadEnvelope<MainnetEthSpec> {
    ExecutionPayloadEnvelope {
        payload: ExecutionPayloadGloas {
            slot_number: Slot::new(TEST_SLOT),
            ..Default::default()
        },
        execution_requests: ExecutionRequestsGloas::default(),
        builder_index: BUILDER_INDEX_SELF_BUILD,
        beacon_block_root: block.canonical_root(),
        parent_beacon_block_root: block.parent_root(),
    }
}

fn context(
    envelope: &ExecutionPayloadEnvelope<MainnetEthSpec>,
    built_locally: bool,
) -> DecidedBlockContext {
    DecidedBlockContext {
        beacon_block_root: envelope.beacon_block_root,
        parent_block_root: envelope.parent_beacon_block_root,
        execution_requests_root: envelope.execution_requests.tree_hash_root(),
        payload_root: envelope.payload.tree_hash_root(),
        builder_index: envelope.builder_index,
        block_hash: envelope.payload.block_hash,
        built_locally,
    }
}

fn domain(harness: &ValidatorStoreTestHarness, domain: Domain) -> Hash256 {
    let epoch = Slot::new(TEST_SLOT).epoch(MainnetEthSpec::slots_per_epoch());
    harness.spec.get_domain(
        epoch,
        domain,
        &harness.spec.fork_at_epoch(epoch),
        harness.genesis_validators_root,
    )
}

fn seed(harness: &ValidatorStoreTestHarness, pubkey: PublicKeyBytes, context: DecidedBlockContext) {
    harness
        .validator_store
        .record_decided_block_context(pubkey, Slot::new(TEST_SLOT), context)
        .unwrap();
}

fn decision(
    setup: &CommitteeSetup,
    pubkey: PublicKeyBytes,
    block: &BeaconBlock<MainnetEthSpec>,
    payload_root: Hash256,
) -> ProposerConsensusData {
    let BeaconBlock::Gloas(block) = block.clone() else {
        panic!("Gloas fixture")
    };
    ProposerConsensusData {
        duty: ValidatorDuty {
            r#type: BEACON_ROLE_PROPOSER,
            pub_key: pubkey,
            slot: Slot::new(TEST_SLOT),
            validator_index: setup.validators[0].index.unwrap(),
            committee_index: 0,
            committee_length: 0,
            committees_at_slot: 0,
            validator_committee_index: 0,
            validator_sync_committee_indices: Default::default(),
        },
        version: DataVersion::from(ForkName::Gloas),
        data_ssz: VariableList::new(
            GloasProposalData {
                block,
                payload_root,
            }
            .as_ssz_bytes(),
        )
        .unwrap(),
    }
}

#[test]
fn doubly_blinded_envelope_matches_independent_progressive_root_vector() {
    // Arrange: retain the fixture checked against consensus-specs pyspec at a5a1bc630.
    let full = ExecutionPayloadEnvelope::<MainnetEthSpec> {
        payload: ExecutionPayloadGloas::default(),
        execution_requests: ExecutionRequestsGloas::default(),
        builder_index: 42,
        beacon_block_root: Hash256::from_low_u64_be(0x1111),
        parent_beacon_block_root: Hash256::from_low_u64_be(0x2222),
    };

    // Act: replace both variable fields with their roots using the production view.
    let blinded = BlindedExecutionPayloadEnvelope::from_full(&full);

    // Assert: both full parity and an independent fixed root survive the second blinding.
    let expected = Hash256::from_slice(
        &hex::decode("9af9a50572381e869605147c3d6220c969d6f12087d94393ec0440660f752c5e").unwrap(),
    );
    assert_eq!(blinded.tree_hash_root(), full.tree_hash_root());
    assert_eq!(blinded.tree_hash_root(), expected);
    assert_eq!(blinded.payload_root, full.payload.tree_hash_root());
    assert_eq!(
        blinded.execution_requests_root,
        full.execution_requests.tree_hash_root()
    );
}

#[tokio::test]
async fn self_build_signs_one_pair_then_callback_only_waits() {
    // Arrange: local contents match the consensus value.
    let (harness, pubkey) = harness();
    let block = block(&harness.spec, BUILDER_INDEX_SELF_BUILD);
    let envelope = envelope(&block);
    let expected_block = SigningData {
        object_root: block.canonical_root(),
        domain: domain(&harness, Domain::BeaconProposer),
    }
    .tree_hash_root();
    let expected_envelope = BlindedExecutionPayloadEnvelope::from_full(&envelope)
        .signing_root(domain(&harness, Domain::BeaconBuilder));

    // Act: the block callback completes before Lighthouse asks for the envelope.
    harness
        .validator_store
        .sign_block(
            pubkey,
            UnsignedBlock::Full(FullBlockContents::Block(block)),
            Slot::new(TEST_SLOT),
            Some(envelope.payload.tree_hash_root()),
        )
        .await
        .unwrap();
    {
        let calls = harness.captured_calls.lock();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].signing_root, expected_block);
        assert_eq!(calls[0].envelope_signing_root, Some(expected_envelope));
        assert_eq!(calls[0].metadata.role, Role::Proposer);
        assert_eq!(calls[0].metadata.kind, PartialSignatureKind::PostConsensus);
        assert!(!calls[0].wait_only);
    }
    let signed = harness
        .validator_store
        .sign_execution_payload_envelope(pubkey, envelope.clone())
        .await
        .unwrap();

    // Assert: publication uses the original contents and does not sign or send a second packet.
    assert_eq!(signed.message, envelope);
    let calls = harness.captured_calls.lock();
    assert_eq!(calls.len(), 2);
    assert!(calls[1].wait_only);
    assert_eq!(calls[1].signing_root, expected_envelope);
    assert_eq!(calls[1].envelope_signing_root, Some(expected_block));
}

#[tokio::test]
async fn external_local_candidate_signs_decided_self_build_without_local_payload() {
    // Arrange: the local builder is external, but QBFT decides another operator's self-build.
    let (setup, pubkey) = committee();
    let spec = gloas_at_genesis_spec();
    let decided_block = block(&spec, BUILDER_INDEX_SELF_BUILD);
    let decided_envelope = envelope(&decided_block);
    let decided = decision(
        &setup,
        pubkey,
        &decided_block,
        decided_envelope.payload.tree_hash_root(),
    );
    let harness = ValidatorStoreTestHarness::new_with_options(
        vec![setup],
        OperatorId(1),
        HarnessOptions {
            decider: MockConsensusDecider::fixed_after_barrier(&decided, 1),
            ..options()
        },
    );
    let expected = BlindedExecutionPayloadEnvelope::from_full(&decided_envelope)
        .signing_root(domain(&harness, Domain::BeaconBuilder));

    // Act: no local payload or later dissemination is supplied.
    harness
        .validator_store
        .sign_block(
            pubkey,
            UnsignedBlock::Full(FullBlockContents::Block(block(&spec, EXTERNAL_BUILDER))),
            Slot::new(TEST_SLOT),
            None,
        )
        .await
        .unwrap();
    let result = harness
        .validator_store
        .sign_execution_payload_envelope(pubkey, decided_envelope)
        .await;

    // Assert: both shares were signed from the decision, while local publication is suppressed.
    assert!(matches!(
        result,
        Err(Error::SpecificError(SpecificError::EnvelopeNotLocal { .. }))
    ));
    let calls = harness.captured_calls.lock();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].envelope_signing_root, Some(expected));
}

#[tokio::test]
async fn decided_external_builder_sends_only_block_share() {
    // Arrange: the local self-build loses to an external builder decision.
    let (setup, pubkey) = committee();
    let spec = gloas_at_genesis_spec();
    let decided = decision(
        &setup,
        pubkey,
        &block(&spec, EXTERNAL_BUILDER),
        Hash256::ZERO,
    );
    let harness = ValidatorStoreTestHarness::new_with_options(
        vec![setup],
        OperatorId(1),
        HarnessOptions {
            decider: MockConsensusDecider::fixed_after_barrier(&decided, 1),
            ..options()
        },
    );
    let local = block(&spec, BUILDER_INDEX_SELF_BUILD);
    let local_envelope = envelope(&local);

    // Act: sign the committee decision, then exercise Lighthouse's local callback.
    harness
        .validator_store
        .sign_block(
            pubkey,
            UnsignedBlock::Full(FullBlockContents::Block(local)),
            Slot::new(TEST_SLOT),
            Some(local_envelope.payload.tree_hash_root()),
        )
        .await
        .unwrap();
    let result = harness
        .validator_store
        .sign_execution_payload_envelope(pubkey, local_envelope)
        .await;

    // Assert: the external decision requires no envelope share or wait.
    assert!(matches!(
        result,
        Err(Error::SpecificError(
            SpecificError::EnvelopeExternalBuild { .. }
        ))
    ));
    let calls = harness.captured_calls.lock();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].envelope_signing_root, None);
}

#[tokio::test]
async fn missing_local_self_build_payload_root_aborts_before_signing() {
    // Arrange: a malformed stateless production response omitted the self-build payload root.
    let (harness, pubkey) = harness();
    let block = block(&harness.spec, BUILDER_INDEX_SELF_BUILD);

    // Act: enter the actual block callback without a root.
    let result = harness
        .validator_store
        .sign_block(
            pubkey,
            UnsignedBlock::Full(FullBlockContents::Block(block)),
            Slot::new(TEST_SLOT),
            None,
        )
        .await;

    // Assert: no invented root or outward signature can follow the malformed response.
    assert!(matches!(
        result,
        Err(Error::SpecificError(SpecificError::MissingLocalPayloadRoot))
    ));
    assert!(harness.captured_calls.lock().is_empty());
}

#[tokio::test]
async fn each_decided_envelope_field_is_required_before_publication_wait() {
    for field in [
        "payload_root",
        "execution_requests_root",
        "builder_index",
        "beacon_block_root",
        "parent_beacon_block_root",
        "block_hash",
    ] {
        // Arrange: start with matching contents and change exactly one decided field.
        let (harness, pubkey) = harness();
        let envelope = envelope(&block(&harness.spec, BUILDER_INDEX_SELF_BUILD));
        let mut context = context(&envelope, true);
        match field {
            "payload_root" => context.payload_root = Hash256::repeat_byte(1),
            "execution_requests_root" => context.execution_requests_root = Hash256::repeat_byte(2),
            "builder_index" => context.builder_index = EXTERNAL_BUILDER,
            "block_hash" => {
                context.block_hash = types::ExecutionBlockHash::from_root(Hash256::repeat_byte(5))
            }
            "beacon_block_root" => context.beacon_block_root = Hash256::repeat_byte(3),
            "parent_beacon_block_root" => context.parent_block_root = Hash256::repeat_byte(4),
            _ => unreachable!(),
        }
        seed(&harness, pubkey, context);

        // Act: try to publish local contents against the mismatching decision.
        let result = harness
            .validator_store
            .sign_execution_payload_envelope(pubkey, envelope)
            .await;

        // Assert: rejection happens before even waiting on a collector.
        assert!(result.is_err(), "must bind {field}");
        assert!(
            harness.captured_calls.lock().is_empty(),
            "must reject {field} before collection"
        );
    }
}

#[tokio::test]
async fn slashable_block_signs_neither_block_nor_envelope_share() {
    // Arrange: protection already records a different block at this slot.
    let (setup, pubkey) = committee();
    let harness = ValidatorStoreTestHarness::new_with_options(
        vec![setup],
        OperatorId(1),
        HarnessOptions {
            disable_slashing_protection: false,
            ..options()
        },
    );
    harness
        .slashing_protection
        .check_and_insert_block_signing_root(
            &pubkey,
            Slot::new(TEST_SLOT),
            Hash256::repeat_byte(9).into(),
        )
        .unwrap();
    let block = block(&harness.spec, BUILDER_INDEX_SELF_BUILD);
    let payload_root = envelope(&block).payload.tree_hash_root();

    // Act: the paired signing path must pass the existing block slashing gate.
    let result = harness
        .validator_store
        .sign_block(
            pubkey,
            UnsignedBlock::Full(FullBlockContents::Block(block)),
            Slot::new(TEST_SLOT),
            Some(payload_root),
        )
        .await;

    // Assert: both shares remain unsent, not merely the conflicting block share.
    assert!(matches!(result, Err(Error::Slashable(_))));
    assert!(harness.captured_calls.lock().is_empty());
}

#[tokio::test(start_paused = true)]
async fn envelope_wait_times_out_without_affecting_completed_block() {
    // Arrange: the block quorum succeeds, then the envelope quorum remains unavailable.
    let (harness, pubkey) = harness();
    let block = block(&harness.spec, BUILDER_INDEX_SELF_BUILD);
    let envelope = envelope(&block);
    let signed_block = harness
        .validator_store
        .sign_block(
            pubkey,
            UnsignedBlock::Full(FullBlockContents::Block(block)),
            Slot::new(TEST_SLOT),
            Some(envelope.payload.tree_hash_root()),
        )
        .await;
    assert!(signed_block.is_ok());
    harness.hang_signature_collection();

    // Act: only the envelope callback waits for its independent threshold.
    let result = harness
        .validator_store
        .sign_execution_payload_envelope(pubkey, envelope)
        .await;

    // Assert: the deadline ends that wait, with one paired send and one wait-only call.
    assert!(result.is_err());
    let calls = harness.captured_calls.lock();
    assert_eq!(calls.len(), 2);
    assert!(!calls[0].wait_only);
    assert!(calls[1].wait_only);
}

#[tokio::test]
async fn envelope_past_payload_deadline_never_waits() {
    // Arrange: valid local contents, but the configured payload deadline has elapsed.
    let (harness, pubkey) = harness();
    let envelope = envelope(&block(&harness.spec, BUILDER_INDEX_SELF_BUILD));
    seed(&harness, pubkey, context(&envelope, true));
    let slot_start = harness.slot_clock.start_of(Slot::new(TEST_SLOT)).unwrap();
    harness
        .slot_clock
        .set_current_time(slot_start + Duration::from_secs(7));

    // Act: exercise the real deadline guard.
    let result = harness
        .validator_store
        .sign_execution_payload_envelope(pubkey, envelope)
        .await;

    // Assert: expiry is checked before registering any collection wait.
    assert!(matches!(
        result,
        Err(Error::SpecificError(
            SpecificError::EnvelopeDeadlinePassed { .. }
        ))
    ));
    assert!(harness.captured_calls.lock().is_empty());
}

#[tokio::test]
async fn same_block_with_different_decided_payload_signs_decision_but_cannot_publish_local_contents()
 {
    // Arrange: block bytes match locally, but the decided payload root belongs to different
    // contents.
    let (setup, pubkey) = committee();
    let spec = gloas_at_genesis_spec();
    let block = block(&spec, BUILDER_INDEX_SELF_BUILD);
    let local_envelope = envelope(&block);
    let mut decided_envelope = local_envelope.clone();
    decided_envelope.payload.block_number = 42;
    let decided = decision(
        &setup,
        pubkey,
        &block,
        decided_envelope.payload.tree_hash_root(),
    );
    let harness = ValidatorStoreTestHarness::new_with_options(
        vec![setup],
        OperatorId(1),
        HarnessOptions {
            decider: MockConsensusDecider::fixed_after_barrier(&decided, 1),
            ..options()
        },
    );
    let expected = BlindedExecutionPayloadEnvelope::from_full(&decided_envelope)
        .signing_root(domain(&harness, Domain::BeaconBuilder));

    // Act: sign the decided roots, then offer the local payload to the publication callback.
    harness
        .validator_store
        .sign_block(
            pubkey,
            UnsignedBlock::Full(FullBlockContents::Block(block)),
            Slot::new(TEST_SLOT),
            Some(local_envelope.payload.tree_hash_root()),
        )
        .await
        .unwrap();
    let result = harness
        .validator_store
        .sign_execution_payload_envelope(pubkey, local_envelope)
        .await;

    // Assert: same block ownership is insufficient when the payload itself differs.
    assert!(matches!(
        result,
        Err(Error::SpecificError(
            SpecificError::EnvelopeBindingMismatch {
                field: "payload_root"
            }
        ))
    ));
    let calls = harness.captured_calls.lock();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].envelope_signing_root, Some(expected));
}

#[tokio::test]
async fn invalid_envelope_callback_inputs_never_wait() {
    for case in ["future_slot", "external_envelope", "missing_decision"] {
        // Arrange: no decided context, with one callback precondition violated per case.
        let (harness, pubkey) = harness();
        let mut envelope = envelope(&block(&harness.spec, BUILDER_INDEX_SELF_BUILD));
        match case {
            "future_slot" => envelope.payload.slot_number = Slot::new(TEST_SLOT + 1),
            "external_envelope" => envelope.builder_index = EXTERNAL_BUILDER,
            _ => {}
        }

        // Act: the callback validates inputs before interacting with collectors.
        let result = harness
            .validator_store
            .sign_execution_payload_envelope(pubkey, envelope)
            .await;

        // Assert: retain the precise error ordering and no outward action.
        match case {
            "future_slot" => assert!(matches!(result, Err(Error::GreaterThanCurrentSlot { .. }))),
            "external_envelope" => assert!(matches!(
                result,
                Err(Error::SpecificError(
                    SpecificError::EnvelopeNotSelfBuild { .. }
                ))
            )),
            _ => assert!(matches!(
                result,
                Err(Error::SpecificError(
                    SpecificError::DecidedRootUnavailable { .. }
                ))
            )),
        }
        assert!(harness.captured_calls.lock().is_empty());
    }
}

#[tokio::test(start_paused = true)]
async fn envelope_wait_uses_configured_payload_deadline() {
    for (payload_due_bps, due_seconds) in [(2500, 3), (7500, 9)] {
        // Arrange: keep collection pending from one second into the slot.
        let (setup, pubkey) = committee();
        let mut spec = (*gloas_at_genesis_spec()).clone();
        spec.payload_due_bps = payload_due_bps;
        let harness = ValidatorStoreTestHarness::new_with_options(
            vec![setup],
            OperatorId(1),
            HarnessOptions {
                spec: std::sync::Arc::new(spec.compute_derived_values::<MainnetEthSpec>()),
                collector_hangs: true,
                ..options()
            },
        );
        let envelope = envelope(&block(&harness.spec, BUILDER_INDEX_SELF_BUILD));
        seed(&harness, pubkey, context(&envelope, true));
        let slot_start = harness.slot_clock.start_of(Slot::new(TEST_SLOT)).unwrap();
        harness
            .slot_clock
            .set_current_time(slot_start + Duration::from_secs(1));
        let start = tokio::time::Instant::now();

        // Act: Tokio advances only to the production callback's actual timeout.
        let result = harness
            .validator_store
            .sign_execution_payload_envelope(pubkey, envelope)
            .await;

        // Assert: #1310's configured deadline survives the fold, including non-default values.
        assert!(result.is_err());
        assert_eq!(start.elapsed(), Duration::from_secs(due_seconds - 1));
        let calls = harness.captured_calls.lock();
        assert_eq!(calls.len(), 1);
        assert!(calls[0].wait_only);
    }
}
