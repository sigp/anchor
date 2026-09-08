//! Regression coverage for issue #1292 at the first Gloas slot.
//!
//! These tests exercise Anchor's actual consensus callers and Phase 3 scheduler. The scheduler
//! test supplies assignments directly, so it does not cover beacon API fetching or duty discovery.

use std::{collections::HashMap, sync::Arc, time::Duration};

use bls::{FixedBytesExtended, Signature};
use fork::Fork;
use futures::{FutureExt, StreamExt};
use qbft_manager::TimeoutMode;
use slot_clock::{ManualSlotClock, SlotClock};
use ssv_types::{
    OperatorId,
    consensus::{AggregatorCommitteeConsensusData, AssignedAggregator},
};
use ssz_types::VariableList;
use tokio::time::Instant;
use types::{Attestation, Checkpoint, Epoch, EthSpec, Hash256, MainnetEthSpec, Slot};
use validator_store::ValidatorStore;

use super::common::*;
use crate::{AggregationAssignments, metadata_service::run_aggregation_publisher};

const OUR_OPERATOR_ID: OperatorId = OperatorId(1);
const COMMITTEE_INDEX: usize = 0;
const VALIDATOR_INDEX: usize = 0;
const VALIDATOR_COUNT: usize = 1;
const SYNC_SUBCOMMITTEE: u64 = 0;
const GLOAS_EPOCH: Epoch = Epoch::new(1);
const PRE_GLOAS_OFFSET: Duration = Duration::from_millis(8_000);
const GLOAS_OFFSET: Duration = Duration::from_millis(6_000);
const BEFORE_DEADLINE: Duration = Duration::from_millis(1);

fn boundary_slots() -> [(Slot, Duration); 2] {
    let first_gloas_slot = GLOAS_EPOCH.start_slot(MainnetEthSpec::slots_per_epoch());
    [
        (first_gloas_slot - 1, PRE_GLOAS_OFFSET),
        (first_gloas_slot, GLOAS_OFFSET),
    ]
}

fn timing_harness(active_fork: Fork) -> ValidatorStoreTestHarness {
    ValidatorStoreTestHarness::new_with_options(
        vec![create_primary_committee_setup(VALIDATOR_COUNT)],
        OUR_OPERATOR_ID,
        HarnessOptions {
            spec: gloas_at_epoch_spec(GLOAS_EPOCH),
            active_fork,
            ..Default::default()
        },
    )
}

/// Legacy aggregate callbacks must use the duty slot's fork for their cumulative round origin.
#[tokio::test(start_paused = true)]
async fn legacy_aggregate_origin_switches_at_gloas_boundary() {
    for (slot, expected_offset) in boundary_slots() {
        // Arrange: use the same mainnet spec on both sides of the fork boundary.
        let harness = timing_harness(Fork::Alan);
        harness.slot_clock.set_slot(slot.as_u64());
        let mut aggregate = harness.create_aggregate(COMMITTEE_INDEX, VALIDATOR_INDEX);
        aggregate.aggregate = Attestation::empty_for_signing(
            COMMITTEE_INDEX as u64,
            VALIDATOR_COUNT,
            slot,
            Hash256::zero(),
            Checkpoint::default(),
            Checkpoint {
                epoch: slot.epoch(MainnetEthSpec::slots_per_epoch()),
                root: Hash256::zero(),
            },
            false,
            &harness.spec,
        )
        .expect("test aggregate should match the duty slot's fork");
        let slot_start = Instant::now();

        // Act: run the real Lighthouse callback through signing and the consensus decider.
        let results = harness.collect_aggregates(vec![aggregate]).await;

        // Assert: completion and the exact origin supplied to QBFT, without invoking a getter
        // to calculate the expected value.
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].as_ref().expect("aggregate should sign").len(), 1);
        assert_eq!(
            harness.captured_consensus_timeouts.lock().as_slice(),
            [TimeoutMode::SlotTime {
                round_deadline_origin: slot_start + expected_offset,
            }],
            "legacy aggregate origin is wrong for slot {slot}",
        );
    }
}

/// Boole's shared committee decision must use the same origin as its Phase 3 scheduler.
#[tokio::test(start_paused = true)]
async fn boole_aggregator_committee_origin_switches_at_gloas_boundary() {
    for (slot, expected_offset) in boundary_slots() {
        // Arrange: one selected sync contributor gives the committee a valid worklist.
        let harness = timing_harness(Fork::Boole);
        harness.slot_clock.set_slot(slot.as_u64());
        let validator = harness.validator_metadata(COMMITTEE_INDEX, VALIDATOR_INDEX);
        let (_, cluster) = harness
            .validator_store
            .get_validator_and_cluster(validator.public_key)
            .expect("test validator should belong to its cluster");
        let mut contribution =
            harness.create_contribution(COMMITTEE_INDEX, VALIDATOR_INDEX, SYNC_SUBCOMMITTEE);
        contribution.contribution.slot = slot;
        let data = AggregatorCommitteeConsensusData {
            version: harness
                .spec
                .fork_name_at_slot::<MainnetEthSpec>(slot)
                .into(),
            aggregators: VariableList::empty(),
            aggregator_committee_indexes: VariableList::empty(),
            aggregated_attestations: VariableList::empty(),
            contributors: VariableList::new(vec![AssignedAggregator {
                validator_index: validator
                    .index
                    .expect("test validator should have an index"),
                selection_proof: Signature::empty(),
                committee_index: SYNC_SUBCOMMITTEE,
            }])
            .expect("one contributor should fit"),
            sync_committee_contributions: VariableList::new(vec![contribution.contribution])
                .expect("one contribution should fit"),
        };
        let slot_start = Instant::now();

        // Act: call the consensus path used by the Boole post-consensus execution.
        let decided = harness
            .validator_store
            .run_aggregator_committee_consensus(cluster.committee_id(), slot, &cluster, &data)
            .await
            .expect("committee consensus should succeed");

        // Assert: both the returned worklist and the captured origin come from the real caller.
        assert_eq!(decided, data);
        assert_eq!(
            harness.captured_consensus_timeouts.lock().as_slice(),
            [TimeoutMode::SlotTime {
                round_deadline_origin: slot_start + expected_offset,
            }],
            "Boole committee origin is wrong for slot {slot}",
        );
    }
}

/// The scheduler is armed before the fork, so reading the current slot's offset instead of the
/// upcoming slot's offset would leave the first Gloas contribution waiting until eight seconds.
/// Driving both clocks together also checks that the published metadata belongs to the new slot.
#[tokio::test(start_paused = true)]
async fn phase3_scheduler_releases_legacy_contributions_at_the_gloas_boundary() {
    // Arrange: start one slot before the last pre-Gloas duty, with no assignments cached.
    let harness = timing_harness(Fork::Alan);
    let [(last_pre_gloas_slot, _), _] = boundary_slots();
    harness
        .slot_clock
        .set_slot((last_pre_gloas_slot - 1).as_u64());
    let publisher_store = Arc::clone(&harness.validator_store);
    let publisher_clock = harness.slot_clock.clone();
    let publisher = tokio::spawn(run_aggregation_publisher::<MainnetEthSpec, _, _>(
        harness.slot_clock.clone(),
        Arc::clone(&harness.spec),
        move || {
            let store = Arc::clone(&publisher_store);
            let slot = publisher_clock
                .now()
                .expect("manual clock should be readable");
            async move {
                let executions = store.update_aggregation_assignments(AggregationAssignments {
                    slot,
                    aggregator_committees: HashMap::new(),
                    multi_sync_aggregators: HashMap::new(),
                    consensus_data_by_ssv_committee: HashMap::new(),
                });
                assert!(
                    executions.is_empty(),
                    "legacy metadata has no Boole worklist"
                );
            }
        },
    ));
    tokio::task::yield_now().await;

    for (index, (slot, expected_offset)) in boundary_slots().into_iter().enumerate() {
        let to_slot_start = harness
            .slot_clock
            .duration_to_next_slot()
            .expect("manual clock should have a next slot");
        advance_in_lockstep(&harness.slot_clock, to_slot_start).await;
        let slot_start = Instant::now();
        let mut contribution =
            harness.create_contribution(COMMITTEE_INDEX, VALIDATOR_INDEX, SYNC_SUBCOMMITTEE);
        contribution.contribution.slot = slot;
        let callback_store = Arc::clone(&harness.validator_store);
        let callback = tokio::spawn(async move {
            callback_store
                .sign_sync_committee_contributions(vec![contribution])
                .collect::<SignContributionsResult>()
                .await
        });
        tokio::task::yield_now().await;

        // Act: reach just before the expected trigger, then cross it by one millisecond.
        advance_in_lockstep(&harness.slot_clock, expected_offset - BEFORE_DEADLINE).await;

        // Assert: the real contribution callback still awaits Phase 3 metadata before the trigger.
        assert!(
            !callback.is_finished(),
            "slot {slot} contribution started early"
        );
        assert_eq!(harness.captured_consensus_timeouts.lock().len(), index);
        assert!(
            harness
                .validator_store
                .get_aggregation_assignments(slot)
                .now_or_never()
                .is_none(),
            "slot {slot} assignments were published before their deadline",
        );

        advance_in_lockstep(&harness.slot_clock, BEFORE_DEADLINE).await;
        let assignments = harness
            .validator_store
            .get_aggregation_assignments(slot)
            .now_or_never()
            .expect("Phase 3 must publish at the duty slot's aggregation deadline")
            .expect("published assignments should match the requested slot");
        assert_eq!(assignments.slot, slot);
        let results = callback
            .await
            .expect("contribution callback should complete");
        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0].as_ref().expect("contribution should sign").len(),
            1
        );
        assert_eq!(
            harness.captured_consensus_timeouts.lock()[index],
            TimeoutMode::SlotTime {
                round_deadline_origin: slot_start + expected_offset,
            },
            "legacy contribution origin is wrong for slot {slot}",
        );
    }

    publisher.abort();
}

/// Wall time moves first so a woken scheduler reads the slot that the virtual timer reached.
async fn advance_in_lockstep(slot_clock: &ManualSlotClock, duration: Duration) {
    slot_clock.advance_time(duration);
    tokio::time::advance(duration).await;
    tokio::task::yield_now().await;
}
