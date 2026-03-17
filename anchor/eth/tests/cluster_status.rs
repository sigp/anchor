//! Focused cluster-status event coverage for the per-event commit model.
//!
//! These tests cover:
//! - `ClusterLiquidated` updating committed cluster state and progress
//! - `ClusterReactivated` restoring committed cluster state and progress

use alloy::primitives::{Address, Bytes};
use eth::util::compute_cluster_id;

mod common;

use database::UniqueIndex;

use common::*;

async fn setup_cluster_via_events() -> (ProcessorFixture, ssv_types::ClusterId, Address, Vec<u64>) {
    let mut test = ProcessorFixture::new_empty();
    let owner = Address::random();
    let operator_ids = vec![1u64, 2, 3, 4];
    let cluster_id = compute_cluster_id(owner, &operator_ids);

    let operator_logs = operator_ids
        .iter()
        .enumerate()
        .map(|(index, operator_id)| {
            let mut log = create_operator_added_log(
                *operator_id,
                Address::random(),
                create_valid_rsa_public_key_bytes(),
                1000,
            );
            log.block_number = Some(130);
            log.log_index = Some(index as u64);
            log
        })
        .collect();
    test.processor
        .process_logs(operator_logs, true, 130)
        .expect("Operator setup should succeed");

    let (shares, validator_pubkey) =
        create_valid_shares_data_for_owner_and_nonce(&operator_ids, owner, 0);
    let validator_log = create_validator_added_log(
        owner,
        operator_ids.clone(),
        Bytes::from(validator_pubkey.serialize().to_vec()),
        shares,
    );
    test.processor
        .process_logs(vec![validator_log], true, 131)
        .expect("Validator setup should succeed");
    let _ = test.index_sync_rx.recv().await;

    (test, cluster_id, owner, operator_ids)
}

#[tokio::test]
/// `ClusterLiquidated` should mark the cluster as liquidated in both SQLite and the post-commit
/// in-memory read model, then advance the processed block.
async fn test_cluster_liquidated_event_updates_status_and_progress() {
    setup_tracing();

    // Arrange: build the cluster via real events so the event-derived cluster id matches the
    // stored cluster row.
    let (test, cluster_id, owner, operator_ids) = setup_cluster_via_events().await;
    let log = create_cluster_liquidated_log(owner, operator_ids);

    // Act: process the liquidation event.
    test.processor
        .process_logs(vec![log], true, 12390)
        .expect("ClusterLiquidated should commit successfully");

    // Assert: both committed views now expose the liquidated cluster state.
    let state = test.processor.db.state();
    let cluster = state
        .clusters()
        .get_by(&cluster_id)
        .expect("Cluster should still exist after liquidation");
    assert!(cluster.liquidated, "Cluster should be marked as liquidated");
    assert_eq!(test.processor.db.state().get_last_processed_block(), 12390);
    assert_eq!(test.processor.db.state().get_last_processed_event(), None);
}

#[tokio::test]
/// `ClusterReactivated` should clear the liquidated flag again and finish on a coarse
/// processed-block boundary.
async fn test_cluster_reactivated_event_updates_status_and_progress() {
    setup_tracing();

    // Arrange: liquidate an event-derived cluster first so the reactivation has observable work
    // to do.
    let (test, cluster_id, owner, operator_ids) = setup_cluster_via_events().await;
    let liquidation_log = create_cluster_liquidated_log(owner, operator_ids.clone());
    test.processor
        .process_logs(vec![liquidation_log], true, 12391)
        .expect("ClusterLiquidated setup should succeed");

    let reactivation_log = create_cluster_reactivated_log(owner, operator_ids);

    // Act: process the reactivation event.
    test.processor
        .process_logs(vec![reactivation_log], true, 12392)
        .expect("ClusterReactivated should commit successfully");

    // Assert: the same cluster is present again as active and progress advanced normally.
    let state = test.processor.db.state();
    let cluster = state
        .clusters()
        .get_by(&cluster_id)
        .expect("Cluster should still exist after reactivation");
    assert!(!cluster.liquidated, "Cluster should be active again");
    assert_eq!(test.processor.db.state().get_last_processed_block(), 12392);
    assert_eq!(test.processor.db.state().get_last_processed_event(), None);
}
