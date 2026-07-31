//! `Role::EnvelopeProposer` dispatch tests for the Ethereum Gloas (ePBS) fork.
//!
//! These exercise the Gloas-gated routing inside `QbftManager::receive_network_message`:
//! before Gloas an `EnvelopeProposer` message is rejected with `RoleNotActive`;
//! at or after Gloas it spawns an `EnvelopeConsensusData` instance keyed by
//! `EnvelopeProposerInstanceId`, leaving the `ProposerConsensusData` map
//! untouched. As in `gloas_dispatch_tests`, `DashMap` entries are inserted
//! synchronously inside `get_or_spawn_instance`, so map sizes are deterministic
//! immediately after `receive_network_message` returns.

use bls::PublicKeyBytes;
use ssv_types::{
    consensus::{EnvelopeConsensusData, ProposerConsensusData, QbftMessage, QbftMessageType},
    message::SignedSSVMessage,
};

use super::{
    gloas_dispatch_tests::{TEST_SLOT_HEIGHT, build_manager, spec_with_gloas},
    setup::setup_test,
    *,
};
use crate::{EnvelopeProposerInstanceId, ProposerInstanceId, ValidatorDutyKind};

/// Build a `Role::EnvelopeProposer` (`SignedSSVMessage`, `QbftMessage`) pair at the given slot
/// height for the given duty executor. Mirrors
/// `gloas_dispatch_tests::build_committee_message`.
fn build_envelope_message(
    slot_height: u64,
    executor: &DutyExecutor,
) -> (SignedSSVMessage, QbftMessage) {
    build_signed_consensus_pair(
        Role::EnvelopeProposer,
        executor,
        QbftMessageType::Proposal,
        slot_height,
    )
}

/// Before Gloas (here: never scheduled), an `EnvelopeProposer` message must be rejected with
/// `RoleNotActive` for either duty-executor spelling, leaving the envelope and proposer maps
/// empty.
///
/// `MessageId::new` starts from a zeroed 56-byte buffer and writes the executor bytes in place:
/// `DutyExecutor::Validator` fills bytes 8..56, `DutyExecutor::Committee` fills bytes 24..56.
/// Because `PublicKeyBytes::empty()` and `CommitteeId([0; 32])` are both all-zero, each executor
/// writes ONLY zeros into an already-zero buffer, so role 9 (`Role::EnvelopeProposer`) encodes to
/// the identical 56-byte `MessageId` regardless of the executor passed in.
///
/// `MessageId::duty_executor` then selects the executor purely from the role, and role 9 is
/// hard-wired to the `Validator` arm (bytes 8..56). The network receive path always enters the
/// `Some(DutyExecutor::Validator(_))` branch and hits the `Some(Role::EnvelopeProposer)` arm,
/// which checks `gloas_enabled_at_slot` and returns `QbftError::RoleNotActive` when Gloas is
/// inactive. A committee-executor `EnvelopeProposer` is therefore unconstructable, and the
/// `Role::EnvelopeProposer` listing in the `DutyExecutor::Committee` arm is unreachable for
/// role 9 — it exists only for match exhaustiveness, so its `InconsistentMessageId` can never
/// fire here.
#[tokio::test]
async fn envelope_proposer_rejected_before_gloas() {
    // Arrange
    let setup = setup_test(1);
    let manager = build_manager(&setup, spec_with_gloas(None));

    // Make the byte-identity invariant explicit: both executors encode to the same 56 bytes.
    let validator_msg_id = MessageId::new(
        &DomainType([0; 4]),
        Role::EnvelopeProposer,
        &DutyExecutor::Validator(PublicKeyBytes::empty()),
    );
    let committee_msg_id = MessageId::new(
        &DomainType([0; 4]),
        Role::EnvelopeProposer,
        &DutyExecutor::Committee(CommitteeId([0; 32])),
    );
    assert_eq!(
        validator_msg_id.as_ref(),
        committee_msg_id.as_ref(),
        "validator- and committee-executor `EnvelopeProposer` must encode to the same 56 bytes for role 9"
    );

    // Act + Assert: both executor variants route through the same `Validator`/`EnvelopeProposer`
    // arm and return `RoleNotActive` before Gloas, never `InconsistentMessageId`.
    for executor in [
        DutyExecutor::Validator(PublicKeyBytes::empty()),
        DutyExecutor::Committee(CommitteeId([0; 32])),
    ] {
        let (signed_msg, qbft_message) = build_envelope_message(TEST_SLOT_HEIGHT, &executor);
        let result = manager.receive_network_message(
            signed_msg,
            qbft_message,
            unexpected_proposer_duty_lookup,
        );
        assert!(
            matches!(result, Err(QbftError::RoleNotActive)),
            "`EnvelopeProposer` always decodes as `Validator` and must be `RoleNotActive` pre-Gloas, got: {result:?}"
        );
    }

    assert_eq!(
        manager.envelope_consensus_data_instances.len(),
        0,
        "envelope_consensus_data_instances must stay empty pre-Gloas"
    );
    assert_eq!(
        manager.proposer_consensus_data_instances.len(),
        0,
        "proposer_consensus_data_instances must stay empty pre-Gloas"
    );
}

/// With Gloas active from genesis, an `EnvelopeProposer` message must spawn an
/// `EnvelopeConsensusData` instance and leave the `ProposerConsensusData` map untouched.
#[tokio::test]
async fn envelope_proposer_routes_to_envelope_map_at_gloas() {
    // Arrange
    let setup = setup_test(1);
    let manager = build_manager(&setup, spec_with_gloas(Some(0)));
    let (signed_msg, qbft_message) = build_envelope_message(
        TEST_SLOT_HEIGHT,
        &DutyExecutor::Validator(PublicKeyBytes::empty()),
    );

    // Act
    let result = manager.receive_network_message(signed_msg, qbft_message, assigned_proposer_duty);

    // Assert
    assert!(
        result.is_ok(),
        "network receive should succeed at Gloas, got: {result:?}"
    );
    assert_eq!(
        manager.envelope_consensus_data_instances.len(),
        1,
        "EnvelopeProposer message at Gloas must spawn an EnvelopeConsensusData instance"
    );
    assert_eq!(
        manager.proposer_consensus_data_instances.len(),
        0,
        "EnvelopeProposer message at Gloas must NOT touch the ProposerConsensusData map"
    );
}

/// Pin the Gloas activation boundary for `EnvelopeProposer` routing: the last pre-Gloas slot is
/// rejected with `RoleNotActive`, the first Gloas slot spawns an envelope instance. An off-by-one
/// in `gloas_enabled_at_slot` would flip exactly one of the assertions below.
#[tokio::test]
async fn envelope_proposer_routes_at_gloas_activation_boundary() {
    const GLOAS_ACTIVATION_EPOCH: u64 = 5;
    const SLOTS_PER_EPOCH: u64 = 32;

    // Arrange
    let setup = setup_test(1);
    let manager = build_manager(&setup, spec_with_gloas(Some(GLOAS_ACTIVATION_EPOCH)));
    let validator_executor = DutyExecutor::Validator(PublicKeyBytes::empty());

    // Act + Assert: the last pre-Gloas slot is rejected and inserts nothing.
    let last_pre_gloas = GLOAS_ACTIVATION_EPOCH * SLOTS_PER_EPOCH - 1;
    let (signed_msg, qbft_message) = build_envelope_message(last_pre_gloas, &validator_executor);
    let result =
        manager.receive_network_message(signed_msg, qbft_message, unexpected_proposer_duty_lookup);
    assert!(
        matches!(result, Err(QbftError::RoleNotActive)),
        "the last pre-Gloas slot must be rejected with `RoleNotActive`, got: {result:?}"
    );
    assert_eq!(
        manager.envelope_consensus_data_instances.len(),
        0,
        "no envelope instance may be spawned for the last pre-Gloas slot"
    );

    // Act + Assert: the first Gloas slot routes and spawns an instance.
    let first_gloas = GLOAS_ACTIVATION_EPOCH * SLOTS_PER_EPOCH;
    let (signed_msg, qbft_message) = build_envelope_message(first_gloas, &validator_executor);
    let result = manager.receive_network_message(signed_msg, qbft_message, assigned_proposer_duty);
    assert!(
        result.is_ok(),
        "the first Gloas slot must route successfully, got: {result:?}"
    );
    assert_eq!(
        manager.envelope_consensus_data_instances.len(),
        1,
        "the first Gloas slot must spawn exactly one envelope instance"
    );
}

/// `EnvelopeConsensusData::message_id` must produce a validator-scoped `Role::EnvelopeProposer`
/// id that differs byte-wise from `ProposerConsensusData`'s block-proposal id for the same
/// validator, keeping envelope and block duties distinct on the wire. A non-zero public key is
/// required: with an all-zero key the executor round-trip is vacuous (see the byte-identity note
/// on `envelope_proposer_rejected_before_gloas`).
#[test]
fn envelope_proposer_message_id_is_validator_scoped() {
    // Arrange
    let domain = DomainType([0; 4]);
    let validator_pubkey = super::validator_pubkey(0xAB);
    let instance_height: InstanceHeight = (TEST_SLOT_HEIGHT as usize).into();
    let envelope_id = EnvelopeProposerInstanceId {
        validator: validator_pubkey,
        instance_height,
    };
    let proposer_id = ProposerInstanceId {
        validator: validator_pubkey,
        duty: ValidatorDutyKind::Proposal,
        instance_height,
    };

    // Act
    let envelope_msg_id =
        <EnvelopeConsensusData as QbftDecidable<types::MainnetEthSpec>>::message_id(
            &domain,
            &envelope_id,
        );
    let proposer_msg_id =
        <ProposerConsensusData as QbftDecidable<types::MainnetEthSpec>>::message_id(
            &domain,
            &proposer_id,
        );

    // Assert
    assert_eq!(
        envelope_msg_id.role(),
        Some(Role::EnvelopeProposer),
        "envelope message id must carry Role::EnvelopeProposer"
    );
    assert_eq!(
        envelope_msg_id.duty_executor(),
        Some(DutyExecutor::Validator(validator_pubkey)),
        "envelope message id must resolve to the validator duty executor"
    );
    assert_ne!(
        envelope_msg_id.as_ref(),
        proposer_msg_id.as_ref(),
        "envelope and block-proposal message ids must differ for the same validator"
    );
}
