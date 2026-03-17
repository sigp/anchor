//! Focused fee-recipient event coverage for the per-event commit model.
//!
//! These tests cover:
//! - `FeeRecipientAddressUpdated` committing the owner-level override
//! - already-materialized clusters reflecting the new fee recipient after commit

use alloy::primitives::Address;
use database::UniqueIndex;

mod common;

use common::*;

#[tokio::test]
/// `FeeRecipientAddressUpdated` should update both the durable owner override and the
/// already-materialized cluster view, then advance progress on the processed block boundary.
async fn test_fee_recipient_updated_event_updates_owner_and_cluster_views() {
    setup_tracing();

    // Arrange: start from a seeded cluster so the owner already has a materialized cluster view.
    let test = ProcessorFixture::new();
    let owner = test.cluster.owner;
    let new_fee_recipient = Address::random();
    let log = create_fee_recipient_updated_log(owner, new_fee_recipient);

    // Act: process the fee-recipient update event.
    test.processor
        .process_logs(vec![log], true, 12393)
        .expect("FeeRecipientAddressUpdated should commit successfully");

    // Assert: the owner override and cluster read model now expose the new fee recipient.
    let stored_override = test
        .processor
        .db
        .fee_recipient_for_owner(&owner)
        .expect("Fee-recipient lookup should succeed");
    assert_eq!(
        stored_override,
        Some(new_fee_recipient),
        "Owner-level fee-recipient override should be committed",
    );

    let state = test.processor.db.state();
    let cluster = state
        .clusters()
        .get_by(&test.cluster.cluster_id)
        .expect("Cluster should still exist after fee-recipient update");
    assert_eq!(
        cluster.fee_recipient, new_fee_recipient,
        "Materialized cluster view should reflect the committed owner override",
    );
    assert_eq!(test.processor.db.state().get_last_processed_block(), 12393);
    assert_eq!(test.processor.db.state().get_last_processed_event(), None);
}
