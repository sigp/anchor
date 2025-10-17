use ssv_types::{Cluster, OperatorId};
use types::Address;

use crate::test_utils::{InMemoryTestFixture, assertions, generators};

#[cfg(test)]
mod cluster_database_tests {
    use super::*;

    #[test]
    // Test inserting a cluster into the database
    fn test_insert_retrieve_cluster() {
        let fixture = InMemoryTestFixture::new();

        let mut conn = fixture.db.connection().unwrap();
        let tx = conn.transaction().unwrap();

        assertions::cluster::exists_in_db(&fixture.cluster, &tx);
        assertions::cluster::exists_in_memory(&fixture.db, &fixture.cluster);
        assertions::validator::exists_in_memory(&fixture.db, &fixture.validator);
        assertions::validator::exists_in_db(&fixture.validator, &tx);
        assertions::share::exists_in_db(&fixture.validator.public_key, &fixture.shares, &tx);
    }

    #[test]
    // Test deleting the last validator from a cluster and make sure the metadata,
    // cluster, cluster members, and shares are all cleaned up
    fn test_delete_last_validator() {
        let fixture = InMemoryTestFixture::new();
        let pubkey = fixture.validator.public_key;

        let mut conn = fixture.db.connection().unwrap();
        let tx = conn.transaction().unwrap();

        assert!(fixture.db.delete_validator(&pubkey, &tx).is_ok());

        // Since there was only one validator in the cluster, everything should be removed
        assertions::cluster::exists_not_in_db(fixture.cluster.cluster_id, &tx);
        assertions::cluster::exists_not_in_memory(&fixture.db, fixture.cluster.cluster_id);
        assertions::validator::exists_not_in_db(&fixture.validator, &tx);
        assertions::validator::exists_not_in_memory(&fixture.db, &fixture.validator);
        assertions::share::exists_not_in_db(&pubkey, &tx);
        assertions::share::exists_not_in_memory(&fixture.db, &pubkey);
    }

    #[test]
    // Test updating the fee recipient
    fn test_update_fee_recipient() {
        let fixture = InMemoryTestFixture::new();
        let new_fee_recipient = Address::random();

        let mut conn = fixture.db.connection().unwrap();
        let tx = conn.transaction().unwrap();

        // Update fee recipient
        assert!(
            fixture
                .db
                .update_fee_recipient(fixture.cluster.owner, new_fee_recipient, &tx)
                .is_ok()
        );

        // Create expected cluster state for assertions
        let expected_cluster = Cluster {
            fee_recipient: new_fee_recipient,
            ..fixture.cluster.clone()
        };
        assertions::cluster::exists_in_db(&expected_cluster, &tx);
        assertions::cluster::exists_in_memory(&fixture.db, &expected_cluster);
    }

    #[test]
    // Try inserting a cluster that does not already have registers operators in the database
    fn test_insert_cluster_without_operators() {
        let fixture = InMemoryTestFixture::new_empty();
        let cluster = generators::cluster::random(4);
        let metadata = generators::validator::random_metadata(cluster.cluster_id);
        let shares = vec![generators::share::random(
            cluster.cluster_id,
            OperatorId(1),
            &fixture.validator.public_key,
        )];
        let mut conn = fixture.db.connection().unwrap();
        let tx = conn.transaction().unwrap();
        fixture
            .db
            .insert_validator(cluster, &metadata, shares, &tx)
            .expect_err("Insertion should fail");
    }

    #[test]
    // Test updating the operational status of the cluster
    fn test_update_cluster_status() {
        let fixture = InMemoryTestFixture::new();

        let mut conn = fixture.db.connection().unwrap();
        let tx = conn.transaction().unwrap();

        // Test updating to liquidated
        fixture
            .db
            .update_status(fixture.cluster.cluster_id, true, &tx)
            .expect("Failed to update cluster status");

        // Create expected cluster state for assertions
        let expected_cluster = Cluster {
            liquidated: true,
            ..fixture.cluster.clone()
        };
        assertions::cluster::exists_in_db(&expected_cluster, &tx);
        assertions::cluster::exists_in_memory(&fixture.db, &expected_cluster);
    }

    #[test]
    // Test inserting a cluster that already exists
    fn test_duplicate_cluster_insert() {
        let fixture = InMemoryTestFixture::new();
        let mut conn = fixture.db.connection().unwrap();
        let tx = conn.transaction().unwrap();
        fixture
            .db
            .insert_validator(
                fixture.cluster.clone(),
                &fixture.validator,
                fixture.shares.clone(),
                &tx,
            )
            .expect_err("Expected failure when inserting cluster that already exists");
    }

    #[test]
    // Test that we can properly track the fee recipient for an owner
    fn test_fetch_fee_recipient() {
        let fixture = InMemoryTestFixture::new();

        let mut conn = fixture.db.connection().unwrap();
        let tx = conn.transaction().unwrap();

        // Confirm that the fee recipient was inserted when the cluster was made
        let fee_recipient = fixture
            .db
            .fee_recipient_for_owner(&fixture.cluster.owner, &tx)
            .unwrap();
        assert_eq!(fee_recipient, Some(fixture.cluster.fee_recipient));

        // Update fee recipient
        let new_fee_recipient = Address::random();
        assert!(
            fixture
                .db
                .update_fee_recipient(fixture.cluster.owner, new_fee_recipient, &tx)
                .is_ok()
        );

        // Create expected cluster state for assertions
        let expected_cluster = Cluster {
            fee_recipient: new_fee_recipient,
            ..fixture.cluster.clone()
        };
        assertions::cluster::exists_in_db(&expected_cluster, &tx);
        assertions::cluster::exists_in_memory(&fixture.db, &expected_cluster);

        // Confirm that we have set the correct fee recipient for the owner
        let stored_fee_recipient = fixture
            .db
            .fee_recipient_for_owner(&fixture.cluster.owner, &tx)
            .unwrap();
        assert_eq!(stored_fee_recipient, Some(new_fee_recipient));
    }

    #[test]
    // Test that fee_recipient_for_owner handles NULL values correctly after BUMP_NONCE
    fn test_fee_recipient_null_handling() {
        let fixture = InMemoryTestFixture::new_empty();
        let owner = Address::random();

        let mut conn = fixture.db.connection().unwrap();
        let tx = conn.transaction().unwrap();

        // Initially, the owner doesn't exist, so fee_recipient should be None
        let fee_recipient = fixture.db.fee_recipient_for_owner(&owner, &tx).unwrap();
        assert_eq!(fee_recipient, None);

        // Call BUMP_NONCE, which creates an entry with owner and nonce but NULL fee_recipient
        let nonce = fixture.db.bump_and_get_nonce(&owner, &tx).unwrap();
        assert_eq!(nonce, 0);

        // Now fee_recipient_for_owner should handle the NULL value and return None
        let fee_recipient_after_bump = fixture.db.fee_recipient_for_owner(&owner, &tx).unwrap();
        assert_eq!(fee_recipient_after_bump, None);

        // Set a fee recipient and verify it works
        let test_fee_recipient = Address::random();
        fixture
            .db
            .update_fee_recipient(owner, test_fee_recipient, &tx)
            .unwrap();

        let fee_recipient_after_update = fixture.db.fee_recipient_for_owner(&owner, &tx).unwrap();
        assert_eq!(fee_recipient_after_update, Some(test_fee_recipient));
    }

    #[test]
    // Test that nonce progression is consistent regardless of operation order
    fn test_nonce_consistency_different_operation_orders() {
        let fixture = InMemoryTestFixture::new_empty();
        let owner1 = Address::random();
        let owner2 = Address::random();

        let mut conn = fixture.db.connection().unwrap();
        let tx = conn.transaction().unwrap();

        // Scenario 1: Owner1 sets fee_recipient first, then BUMP_NONCE
        let fee_recipient1 = Address::random();
        fixture
            .db
            .update_fee_recipient(owner1, fee_recipient1, &tx)
            .unwrap();

        // Now bump the nonce
        fixture.db.bump_and_get_nonce(&owner1, &tx).unwrap();

        // Scenario 2: Owner2 calls BUMP_NONCE first, then sets fee_recipient
        fixture.db.bump_and_get_nonce(&owner2, &tx).unwrap();

        let fee_recipient2 = Address::random();
        fixture
            .db
            .update_fee_recipient(owner2, fee_recipient2, &tx)
            .unwrap();

        tx.commit().unwrap();
        drop(conn);

        let nonce1 = fixture.db.get_nonce_for_owner(owner1).unwrap();
        let nonce2 = fixture.db.get_nonce_for_owner(owner2).unwrap();

        assert_eq!(
            nonce1, nonce2,
            "Nonce should be consistent regardless of operation order"
        );
    }
}
