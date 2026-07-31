use std::cell::Cell;

use bls::PublicKeyBytes;
use ssv_types::consensus::{EnvelopeConsensusData, ProposerConsensusData, QbftData};
use ssz::Encode;
use tokio::time::timeout;

use super::{
    gloas_dispatch_tests::{TEST_SLOT_HEIGHT, build_manager, spec_with_gloas},
    setup::setup_test,
    *,
};
use crate::{
    EnvelopeProposerInstanceId, ProposerInstanceId, QbftDispatchOutcome, ValidatorDutyKind,
};

fn build_validator_message(
    role: Role,
    message_type: QbftMessageType,
    height: u64,
    validator: PublicKeyBytes,
) -> (SignedSSVMessage, QbftMessage) {
    build_signed_consensus_pair(
        role,
        &DutyExecutor::Validator(validator),
        message_type,
        height,
    )
}

fn proposer_instance_id(validator: PublicKeyBytes, height: u64) -> ProposerInstanceId {
    ProposerInstanceId {
        validator,
        duty: ValidatorDutyKind::Proposal,
        instance_height: (height as usize).into(),
    }
}

fn envelope_instance_id(validator: PublicKeyBytes, height: u64) -> EnvelopeProposerInstanceId {
    EnvelopeProposerInstanceId {
        validator,
        instance_height: (height as usize).into(),
    }
}

fn assert_exact_network_message<D: QbftData>(
    actual: crate::QbftMessage<D>,
    expected_signed: &SignedSSVMessage,
    expected_qbft: &QbftMessage,
) {
    let crate::QbftMessage {
        kind,
        drop_on_finish,
    } = actual;
    assert!(
        drop_on_finish.is_some(),
        "processor permit should travel with the instance message"
    );
    let QbftMessageKind::NetworkMessage(wrapped) = kind else {
        panic!("expected a network message");
    };
    assert_eq!(&wrapped.signed_message, expected_signed);
    assert_eq!(
        wrapped.qbft_message.as_ssz_bytes(),
        expected_qbft.as_ssz_bytes()
    );
}

fn assert_instance_presence(
    manager: &QbftManager<types::MainnetEthSpec, ManualSlotClock>,
    role: Role,
    validator: PublicKeyBytes,
    height: u64,
    expected: bool,
) {
    let present = match role {
        Role::Proposer => manager
            .proposer_consensus_data_instances
            .contains_key(&proposer_instance_id(validator, height)),
        Role::EnvelopeProposer => manager
            .envelope_consensus_data_instances
            .contains_key(&envelope_instance_id(validator, height)),
        _ => panic!("unexpected role in proposer dispatch test: {role:?}"),
    };
    assert_eq!(present, expected, "unexpected {role:?} instance state");
}

async fn assert_unknown_dispatch_is_exact_keyed<D>(
    manager: &QbftManager<types::MainnetEthSpec, ManualSlotClock>,
    role: Role,
    height: u64,
    validator: PublicKeyBytes,
    other_validator: PublicKeyBytes,
    instance_id: D::Id,
    other_instance_id: D::Id,
) where
    D: QbftDecidable<types::MainnetEthSpec>,
{
    let (tx, mut rx) = mpsc::unbounded_channel();
    D::get_map(manager).insert(instance_id, tx);

    let (wrong_signed, wrong_qbft) =
        build_validator_message(role, QbftMessageType::Proposal, height, other_validator);
    let outcome = manager
        .receive_network_message(wrong_signed, wrong_qbft, |slot, validator| {
            assert_eq!(slot, Slot::new(height));
            assert_eq!(validator, &other_validator);
            DutyAssignment::Unknown
        })
        .expect("wrong-key lookup should not fail");
    assert_eq!(outcome, QbftDispatchOutcome::DroppedMissingInstance);
    assert!(
        timeout(Duration::from_millis(50), rx.recv()).await.is_err(),
        "another validator pubkey must not reach the seeded {role:?} instance"
    );
    assert!(!D::get_map(manager).contains_key(&other_instance_id));

    let (correct_signed, correct_qbft) =
        build_validator_message(role, QbftMessageType::Proposal, height, validator);
    let expected_signed = correct_signed.clone();
    let expected_qbft = correct_qbft.clone();
    let outcome = manager
        .receive_network_message(correct_signed, correct_qbft, |slot, pubkey| {
            assert_eq!(slot, Slot::new(height));
            assert_eq!(pubkey, &validator);
            DutyAssignment::Unknown
        })
        .expect("present instance should dispatch");
    assert_eq!(outcome, QbftDispatchOutcome::ProcessorEnqueued);
    let actual = timeout(Duration::from_secs(1), rx.recv())
        .await
        .expect("processor should dispatch promptly")
        .expect("seeded receiver should remain open");
    assert_exact_network_message::<D>(actual, &expected_signed, &expected_qbft);
}

async fn assert_not_assigned_drops_existing<D>(
    manager: &QbftManager<types::MainnetEthSpec, ManualSlotClock>,
    role: Role,
    height: u64,
    validator: PublicKeyBytes,
    instance_id: D::Id,
) where
    D: QbftDecidable<types::MainnetEthSpec>,
    D::Id: Clone,
{
    let (tx, mut rx) = mpsc::unbounded_channel();
    D::get_map(manager).insert(instance_id.clone(), tx);
    let (signed_message, qbft_message) =
        build_validator_message(role, QbftMessageType::Proposal, height, validator);
    let query_count = Cell::new(0);

    let outcome = manager
        .receive_network_message(signed_message, qbft_message, |slot, pubkey| {
            assert_eq!(slot, Slot::new(height));
            assert_eq!(pubkey, &validator);
            query_count.set(query_count.get() + 1);
            DutyAssignment::NotAssigned
        })
        .expect("NotAssigned should be an ordinary drop");

    assert_eq!(outcome, QbftDispatchOutcome::DroppedNotAssigned);
    assert_eq!(query_count.get(), 1);
    assert!(
        timeout(Duration::from_millis(50), rx.recv()).await.is_err(),
        "NotAssigned {role:?} traffic must not reach an existing instance"
    );
    assert!(D::get_map(manager).contains_key(&instance_id));
}

#[tokio::test]
async fn proposer_dispatch_uses_the_latest_assignment() {
    let setup = setup_test(1);
    let manager = build_manager(&setup, spec_with_gloas(Some(0)));
    // Message-validator tests cover the first observation. Keep it in this table to enumerate all
    // accepted validation-to-dispatch transitions while exercising the fresh manager lookup.
    let transitions = [
        (DutyAssignment::Assigned, DutyAssignment::Assigned),
        (DutyAssignment::Assigned, DutyAssignment::Unknown),
        (DutyAssignment::Assigned, DutyAssignment::NotAssigned),
        (DutyAssignment::Unknown, DutyAssignment::Assigned),
        (DutyAssignment::Unknown, DutyAssignment::Unknown),
        (DutyAssignment::Unknown, DutyAssignment::NotAssigned),
    ];

    for (role_offset, role) in [Role::Proposer, Role::EnvelopeProposer]
        .into_iter()
        .enumerate()
    {
        for (transition_offset, (validation_assignment, dispatch_assignment)) in
            transitions.into_iter().enumerate()
        {
            let height =
                TEST_SLOT_HEIGHT + (role_offset * transitions.len() + transition_offset) as u64;
            let validator = validator_pubkey(
                0x10 + (role_offset * transitions.len() + transition_offset) as u8,
            );
            let (signed_message, qbft_message) =
                build_validator_message(role, QbftMessageType::Proposal, height, validator);
            let expected_slot = Slot::new(height);
            assert!(matches!(
                validation_assignment,
                DutyAssignment::Assigned | DutyAssignment::Unknown
            ));
            let query_count = Cell::new(0);

            let outcome = manager
                .receive_network_message(signed_message, qbft_message, |slot, pubkey| {
                    assert_eq!(slot, expected_slot, "unexpected proposer duty slot");
                    assert_eq!(pubkey, &validator, "unexpected proposer duty pubkey");
                    query_count.set(query_count.get() + 1);
                    dispatch_assignment
                })
                .expect("proposer dispatch should not fail");

            let expected_outcome = match dispatch_assignment {
                DutyAssignment::Assigned => QbftDispatchOutcome::ProcessorEnqueued,
                DutyAssignment::Unknown => QbftDispatchOutcome::DroppedMissingInstance,
                DutyAssignment::NotAssigned => QbftDispatchOutcome::DroppedNotAssigned,
            };
            assert_eq!(
                outcome, expected_outcome,
                "{role:?}: {validation_assignment:?} -> {dispatch_assignment:?}"
            );
            assert_eq!(query_count.get(), 1);
            assert_instance_presence(
                &manager,
                role,
                validator,
                height,
                dispatch_assignment == DutyAssignment::Assigned,
            );
        }
    }
}

#[tokio::test]
async fn unknown_dispatch_routes_only_to_exact_validator_instances() {
    let setup = setup_test(1);
    let manager = build_manager(&setup, spec_with_gloas(Some(0)));

    let proposer_height = TEST_SLOT_HEIGHT;
    let proposer_validator = validator_pubkey(0x51);
    let other_proposer_validator = validator_pubkey(0x52);
    assert_unknown_dispatch_is_exact_keyed::<ProposerConsensusData>(
        &manager,
        Role::Proposer,
        proposer_height,
        proposer_validator,
        other_proposer_validator,
        proposer_instance_id(proposer_validator, proposer_height),
        proposer_instance_id(other_proposer_validator, proposer_height),
    )
    .await;

    let envelope_height = TEST_SLOT_HEIGHT + 1;
    let envelope_validator = validator_pubkey(0x61);
    let other_envelope_validator = validator_pubkey(0x62);
    assert_unknown_dispatch_is_exact_keyed::<EnvelopeConsensusData>(
        &manager,
        Role::EnvelopeProposer,
        envelope_height,
        envelope_validator,
        other_envelope_validator,
        envelope_instance_id(envelope_validator, envelope_height),
        envelope_instance_id(other_envelope_validator, envelope_height),
    )
    .await;
}

#[tokio::test]
async fn not_assigned_drops_even_when_an_instance_exists() {
    let setup = setup_test(1);
    let manager = build_manager(&setup, spec_with_gloas(Some(0)));

    let proposer_height = TEST_SLOT_HEIGHT;
    let proposer_validator = validator_pubkey(0x71);
    assert_not_assigned_drops_existing::<ProposerConsensusData>(
        &manager,
        Role::Proposer,
        proposer_height,
        proposer_validator,
        proposer_instance_id(proposer_validator, proposer_height),
    )
    .await;

    let envelope_height = TEST_SLOT_HEIGHT + 1;
    let envelope_validator = validator_pubkey(0x72);
    assert_not_assigned_drops_existing::<EnvelopeConsensusData>(
        &manager,
        Role::EnvelopeProposer,
        envelope_height,
        envelope_validator,
        envelope_instance_id(envelope_validator, envelope_height),
    )
    .await;
}

#[tokio::test]
async fn assigned_proposer_and_other_validator_roles_spawn() {
    let setup = setup_test(1);
    let manager = build_manager(&setup, spec_with_gloas(Some(0)));

    for (offset, (role, duty, proposer_assignment)) in [
        (
            Role::Proposer,
            ValidatorDutyKind::Proposal,
            assigned_proposer_duty as fn(Slot, &PublicKeyBytes) -> DutyAssignment,
        ),
        (
            Role::Aggregator,
            ValidatorDutyKind::Aggregator,
            unexpected_proposer_duty_lookup as fn(Slot, &PublicKeyBytes) -> DutyAssignment,
        ),
        (
            Role::SyncCommittee,
            ValidatorDutyKind::SyncCommitteeAggregator,
            unexpected_proposer_duty_lookup as fn(Slot, &PublicKeyBytes) -> DutyAssignment,
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let height = TEST_SLOT_HEIGHT + offset as u64;
        let validator = validator_pubkey(0x80 + offset as u8);
        let (signed_message, qbft_message) =
            build_validator_message(role, QbftMessageType::Prepare, height, validator);
        let outcome = manager
            .receive_network_message(signed_message, qbft_message, proposer_assignment)
            .expect("network message should dispatch");
        assert_eq!(outcome, QbftDispatchOutcome::ProcessorEnqueued);
        assert!(
            manager
                .proposer_consensus_data_instances
                .contains_key(&ProposerInstanceId {
                    validator,
                    duty,
                    instance_height: (height as usize).into(),
                })
        );
    }
}

#[tokio::test]
async fn pre_gloas_envelope_rejection_precedes_duty_lookup() {
    let setup = setup_test(1);
    let manager = build_manager(&setup, spec_with_gloas(None));
    let (signed_message, qbft_message) = build_validator_message(
        Role::EnvelopeProposer,
        QbftMessageType::Proposal,
        TEST_SLOT_HEIGHT,
        validator_pubkey(0x90),
    );
    let query_count = Cell::new(0);

    let result = manager.receive_network_message(signed_message, qbft_message, |_, _| {
        query_count.set(query_count.get() + 1);
        DutyAssignment::Assigned
    });
    assert!(
        matches!(result, Err(QbftError::RoleNotActive)),
        "pre-Gloas EnvelopeProposer should return RoleNotActive, got {result:?}"
    );
    assert_eq!(query_count.get(), 0);
    assert!(manager.envelope_consensus_data_instances.is_empty());
}
