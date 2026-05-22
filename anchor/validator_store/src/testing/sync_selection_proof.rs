//! Integration tests for sync selection proof production.

use std::collections::HashSet;

use fork::Fork;
use signature_collector::SignatureRequester;
use ssv_types::OperatorId;
use types::{Slot, SyncSubnetId};
use validator_store::ValidatorStore;

use super::common::*;

const OUR_OPERATOR_ID: OperatorId = OperatorId(1);
const COMMITTEE_INDEX: usize = 0;
const VALIDATOR_INDEX: usize = 0;
const SYNC_SUBNET_IDS: [u64; 3] = [0, 1, 2];

/// Pre-Boole sync selection proofs are requested once per sync subnet by Lighthouse.
/// Anchor must still send one `ContributionProofs` envelope for the validator/slot, so each
/// per-subnet signing call should join the same validator-level local batch.
#[tokio::test(flavor = "multi_thread")]
async fn produce_sync_selection_proof_pre_boole_batches_multi_subnet_contribution_proofs() {
    // Arrange
    let committee = create_committee_setup(
        &[OperatorId(1), OperatorId(2), OperatorId(3), OperatorId(4)],
        1,
        0,
    );
    let harness =
        ValidatorStoreTestHarness::new_with_fork(vec![committee], OUR_OPERATOR_ID, Fork::Alan);
    let validator = harness.validator_metadata(COMMITTEE_INDEX, VALIDATOR_INDEX);
    let validator_index = validator.index.expect("test validator should have index");
    let sync_subnets: Vec<_> = SYNC_SUBNET_IDS.into_iter().map(SyncSubnetId::new).collect();
    harness.seed_sync_voting_assignments_for_slot(
        TEST_SLOT,
        vec![(validator_index, sync_subnets.clone())],
    );

    // Act
    for subnet_id in sync_subnets {
        harness
            .validator_store
            .produce_sync_selection_proof(&validator.public_key, Slot::new(TEST_SLOT), subnet_id)
            .await
            .expect("sync selection proof should be produced");
    }

    // Assert
    let captured = harness.captured_calls.lock();
    assert_eq!(
        captured.len(),
        SYNC_SUBNET_IDS.len(),
        "expected one signing request per sync subnet"
    );

    let mut base_hashes = HashSet::new();
    for call in captured.iter() {
        match &call.requester {
            SignatureRequester::SingleValidatorBatch {
                pubkey,
                validator_partial_signature_batch_size,
                base_hash,
            } => {
                assert_eq!(
                    pubkey, &validator.public_key,
                    "all batched requests should target the same validator"
                );
                assert_eq!(
                    *validator_partial_signature_batch_size,
                    SYNC_SUBNET_IDS.len(),
                    "each per-subnet request should wait for the full validator/subnet batch"
                );
                base_hashes.insert(*base_hash);
            }
            other => panic!("expected SignatureRequester::SingleValidatorBatch, got: {other:?}"),
        }
    }

    assert_eq!(
        base_hashes.len(),
        1,
        "all per-subnet requests should use the same local batch id"
    );
}
