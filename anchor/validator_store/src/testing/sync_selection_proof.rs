//! Integration tests for sync selection proof descriptor construction.

use fork::Fork;
use signature_collector::{SignatureRequester, SyncCommitteeBatchEntry};
use ssv_types::OperatorId;
use types::{Slot, SyncSubnetId};
use validator_store::ValidatorStore;

use super::common::*;
use crate::{Error, SpecificError, SyncSelectionProofAssignmentError};

const OUR_OPERATOR_ID: OperatorId = OperatorId(1);
const COMMITTEE_INDEX: usize = 0;
const VALIDATOR_INDEX: usize = 0;

fn harness_with_fork(active_fork: Fork) -> ValidatorStoreTestHarness {
    let committee = create_committee_setup(
        &[OperatorId(1), OperatorId(2), OperatorId(3), OperatorId(4)],
        1,
        0,
    );
    ValidatorStoreTestHarness::new_with_fork(vec![committee], OUR_OPERATOR_ID, active_fork)
}

fn alan_harness() -> ValidatorStoreTestHarness {
    harness_with_fork(Fork::Alan)
}

#[tokio::test(flavor = "multi_thread")]
async fn pre_boole_callbacks_receive_the_same_canonical_descriptor() {
    struct Case {
        position_counts: Vec<(u64, usize)>,
        callbacks: Vec<u64>,
        expected_counts: Vec<(u64, usize)>,
    }

    let cases = [
        Case {
            position_counts: vec![(0, 1)],
            callbacks: vec![0],
            expected_counts: vec![(0, 1)],
        },
        Case {
            position_counts: vec![(0, 2)],
            callbacks: vec![0],
            expected_counts: vec![(0, 2)],
        },
        Case {
            // Insert in reverse numeric order to ensure descriptor construction sorts.
            position_counts: vec![(1, 1), (0, 2)],
            callbacks: vec![1, 0],
            expected_counts: vec![(0, 2), (1, 1)],
        },
    ];

    for case in cases {
        let harness = alan_harness();
        let validator = harness.validator_metadata(COMMITTEE_INDEX, VALIDATOR_INDEX);
        let validator_index = validator
            .index
            .expect("test validator should have an index");
        harness.seed_sync_voting_assignments_for_slot(
            TEST_SLOT,
            vec![(
                validator_index,
                case.position_counts
                    .iter()
                    .map(|(subnet, count)| (SyncSubnetId::new(*subnet), *count))
                    .collect(),
            )],
        );

        for callback in &case.callbacks {
            harness
                .validator_store
                .produce_sync_selection_proof(
                    &validator.public_key,
                    Slot::new(TEST_SLOT),
                    SyncSubnetId::new(*callback),
                )
                .await
                .expect("sync selection proof should be produced");
        }

        let expected_descriptor = case
            .expected_counts
            .iter()
            .map(|(subnet, multiplicity)| SyncCommitteeBatchEntry {
                subnet_id: SyncSubnetId::new(*subnet),
                signing_root: harness
                    .validator_store
                    .compute_sync_selection_root(Slot::new(TEST_SLOT), *subnet),
                multiplicity: *multiplicity,
            })
            .collect::<Vec<_>>();
        let total_multiplicity = expected_descriptor
            .iter()
            .map(|entry| entry.multiplicity)
            .sum::<usize>();

        let captured = harness.captured_calls.lock();
        assert_eq!(captured.len(), case.callbacks.len());
        for (call, callback) in captured.iter().zip(&case.callbacks) {
            let callback_subnet = SyncSubnetId::new(*callback);
            let callback_root = expected_descriptor
                .iter()
                .find(|entry| entry.subnet_id == callback_subnet)
                .expect("callback subnet should be present")
                .signing_root;
            match &call.requester {
                SignatureRequester::SingleValidator { pubkey } if total_multiplicity == 1 => {
                    assert_eq!(*pubkey, validator.public_key);
                    assert_eq!(call.signing_root, callback_root);
                    assert_eq!(call.validator_pubkey, validator.public_key);
                }
                SignatureRequester::SingleValidatorBatch {
                    pubkey,
                    subnet_id,
                    descriptor,
                } if total_multiplicity > 1 => {
                    assert_eq!(*pubkey, validator.public_key);
                    assert_eq!(*subnet_id, callback_subnet);
                    assert_eq!(descriptor, &expected_descriptor);
                    assert_eq!(call.signing_root, callback_root);
                    assert_eq!(call.validator_pubkey, validator.public_key);
                }
                other => panic!("unexpected signature requester: {other:?}"),
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn pre_boole_assignment_validation_reports_precise_errors() {
    struct Case {
        assignments: Option<Vec<(u64, usize)>>,
        callback_subnet: u64,
        expected: Option<SyncSelectionProofAssignmentError>,
    }

    let cases = [
        Case {
            assignments: None,
            callback_subnet: 0,
            expected: None,
        },
        Case {
            assignments: Some(vec![]),
            callback_subnet: 0,
            expected: Some(SyncSelectionProofAssignmentError::Empty),
        },
        Case {
            assignments: Some(vec![(0, 1)]),
            callback_subnet: 1,
            expected: Some(SyncSelectionProofAssignmentError::MissingCallbackSubnet {
                subnet_id: SyncSubnetId::new(1),
            }),
        },
        Case {
            assignments: Some(vec![(4, 1)]),
            callback_subnet: 4,
            expected: Some(SyncSelectionProofAssignmentError::OutOfRangeSubnet {
                subnet_id: SyncSubnetId::new(4),
                subnet_count: 4,
            }),
        },
        Case {
            assignments: Some(vec![(5, 1), (0, 14), (4, 1)]),
            callback_subnet: 0,
            expected: Some(SyncSelectionProofAssignmentError::OutOfRangeSubnet {
                subnet_id: SyncSubnetId::new(4),
                subnet_count: 4,
            }),
        },
        Case {
            assignments: Some(vec![(0, 14)]),
            callback_subnet: 0,
            expected: Some(SyncSelectionProofAssignmentError::TooManyPositions {
                count: 14,
                max: 13,
            }),
        },
        Case {
            assignments: Some(vec![(0, usize::MAX)]),
            callback_subnet: 0,
            expected: Some(SyncSelectionProofAssignmentError::TooManyPositions {
                count: usize::MAX,
                max: 13,
            }),
        },
    ];

    for case in cases {
        let harness = alan_harness();
        let validator = harness.validator_metadata(COMMITTEE_INDEX, VALIDATOR_INDEX);
        let validator_index = validator
            .index
            .expect("test validator should have an index");
        let assignments = case.assignments.map_or_else(Vec::new, |counts| {
            vec![(
                validator_index,
                counts
                    .into_iter()
                    .map(|(subnet, count)| (SyncSubnetId::new(subnet), count))
                    .collect(),
            )]
        });
        harness.seed_sync_voting_assignments_for_slot(TEST_SLOT, assignments);

        let result = harness
            .validator_store
            .produce_sync_selection_proof(
                &validator.public_key,
                Slot::new(TEST_SLOT),
                SyncSubnetId::new(case.callback_subnet),
            )
            .await;

        match case.expected {
            None => assert!(matches!(
                result,
                Err(Error::SpecificError(
                    SpecificError::ValidatorNotInSyncCommittee {
                        validator_pubkey,
                        slot,
                    }
                )) if validator_pubkey == validator.public_key && slot == Slot::new(TEST_SLOT)
            )),
            Some(expected) => assert!(matches!(
                result,
                Err(Error::SpecificError(
                    SpecificError::InvalidSyncSelectionProofAssignment {
                        validator_pubkey,
                        slot,
                        reason,
                    }
                )) if validator_pubkey == validator.public_key
                    && slot == Slot::new(TEST_SLOT)
                    && reason == expected
            )),
        }
        assert!(harness.captured_calls.lock().is_empty());
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn pre_boole_accepts_exactly_thirteen_positions() {
    let harness = alan_harness();
    let validator = harness.validator_metadata(COMMITTEE_INDEX, VALIDATOR_INDEX);
    let validator_index = validator
        .index
        .expect("test validator should have an index");
    let subnet = SyncSubnetId::new(0);
    harness.seed_sync_voting_assignments_for_slot(
        TEST_SLOT,
        vec![(validator_index, vec![(subnet, 13)])],
    );

    harness
        .validator_store
        .produce_sync_selection_proof(&validator.public_key, Slot::new(TEST_SLOT), subnet)
        .await
        .expect("thirteen positions should be accepted");

    let captured = harness.captured_calls.lock();
    assert!(matches!(
        &captured[0].requester,
        SignatureRequester::SingleValidatorBatch { descriptor, .. }
            if descriptor[0].multiplicity == 13
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn boole_keeps_committee_mode_and_counts_unique_subnets() {
    let harness = harness_with_fork(Fork::Boole);
    let validator = harness.validator_metadata(COMMITTEE_INDEX, VALIDATOR_INDEX);
    let validator_index = validator
        .index
        .expect("test validator should have an index");
    let subnet = SyncSubnetId::new(0);
    harness.seed_sync_voting_assignments_for_slot(
        TEST_SLOT,
        vec![(
            validator_index,
            vec![(subnet, 2), (SyncSubnetId::new(1), 1)],
        )],
    );

    harness
        .validator_store
        .produce_sync_selection_proof(&validator.public_key, Slot::new(TEST_SLOT), subnet)
        .await
        .expect("Boole sync selection proof should be produced");

    let captured = harness.captured_calls.lock();
    assert!(matches!(
        &captured[0].requester,
        SignatureRequester::Committee {
            validator_partial_signature_batch_size: 2,
            ..
        }
    ));
}
