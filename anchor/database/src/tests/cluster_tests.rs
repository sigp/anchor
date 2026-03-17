use ssv_types::{Cluster, OperatorId};
use types::Address;

use crate::test_utils::{InMemoryTestFixture, assertions, generators, test_cursor};

#[cfg(test)]
mod cluster_database_tests {
    use super::*;

    #[test]
    // Test inserting a cluster into the database
    fn test_insert_retrieve_cluster() {
        let fixture = InMemoryTestFixture::new();

        assertions::cluster::exists_in_db(&fixture.db, &fixture.cluster);
        assertions::cluster::exists_in_memory(&fixture.db, &fixture.cluster);
        assertions::validator::exists_in_memory(&fixture.db, &fixture.validator);
        assertions::validator::exists_in_db(&fixture.db, &fixture.validator);
        assertions::share::exists_in_db(
            &fixture.db,
            &fixture.validator.public_key,
            &fixture.shares,
        );
    }

    #[test]
    // Test deleting the last validator from a cluster and make sure the metadata,
    // cluster, cluster members, and shares are all cleaned up
    fn test_delete_last_validator() {
        let fixture = InMemoryTestFixture::new();
        let pubkey = fixture.validator.public_key;

        assert!(
            fixture
                .db
                .commit_validator_removed(pubkey, test_cursor(0))
                .is_ok()
        );

        // Since there was only one validator in the cluster, everything should be removed
        assertions::cluster::exists_not_in_db(&fixture.db, fixture.cluster.cluster_id);
        assertions::cluster::exists_not_in_memory(&fixture.db, fixture.cluster.cluster_id);
        assertions::validator::exists_not_in_db(&fixture.db, &fixture.validator);
        assertions::validator::exists_not_in_memory(&fixture.db, &fixture.validator);
        assertions::share::exists_not_in_db(&fixture.db, &pubkey);
        assertions::share::exists_not_in_memory(&fixture.db, &pubkey);
    }

    #[test]
    // Test updating the fee recipient
    fn test_update_fee_recipient() {
        let fixture = InMemoryTestFixture::new();
        let new_fee_recipient = Address::random();

        // Update fee recipient
        assert!(
            fixture
                .db
                .commit_fee_recipient_updated(
                    fixture.cluster.owner,
                    new_fee_recipient,
                    test_cursor(0),
                )
                .is_ok()
        );

        // Create expected cluster state for assertions
        let expected_cluster = Cluster {
            fee_recipient: new_fee_recipient,
            ..fixture.cluster.clone()
        };
        assertions::cluster::exists_in_db(&fixture.db, &expected_cluster);
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
        fixture
            .db
            .commit_validator_added(
                cluster.cluster_id,
                cluster.owner,
                metadata,
                shares,
                test_cursor(0),
            )
            .expect_err("Insertion should fail");
    }

    #[test]
    // Test updating the operational status of the cluster
    fn test_update_cluster_status() {
        let fixture = InMemoryTestFixture::new();

        // Test updating to liquidated
        fixture
            .db
            .commit_cluster_status(fixture.cluster.cluster_id, true, test_cursor(0))
            .expect("Failed to update cluster status");

        // Create expected cluster state for assertions
        let expected_cluster = Cluster {
            liquidated: true,
            ..fixture.cluster.clone()
        };
        assertions::cluster::exists_in_db(&fixture.db, &expected_cluster);
        assertions::cluster::exists_in_memory(&fixture.db, &expected_cluster);
    }

    #[test]
    // Test inserting a cluster that already exists
    fn test_duplicate_cluster_insert() {
        let fixture = InMemoryTestFixture::new();
        fixture
            .db
            .commit_validator_added(
                fixture.cluster.cluster_id,
                fixture.cluster.owner,
                fixture.validator.clone(),
                fixture.shares.clone(),
                test_cursor(0),
            )
            .expect_err("Expected failure when inserting cluster that already exists");
    }

    #[test]
    // Test that we can properly track the fee recipient for an owner
    fn test_fetch_fee_recipient() {
        let fixture = InMemoryTestFixture::new();

        // Confirm that the fee recipient was inserted when the cluster was made
        let fee_recipient = fixture
            .db
            .fee_recipient_for_owner(&fixture.cluster.owner)
            .unwrap();
        assert_eq!(fee_recipient, Some(fixture.cluster.fee_recipient));

        // Update fee recipient
        let new_fee_recipient = Address::random();
        assert!(
            fixture
                .db
                .commit_fee_recipient_updated(
                    fixture.cluster.owner,
                    new_fee_recipient,
                    test_cursor(0),
                )
                .is_ok()
        );

        // Create expected cluster state for assertions
        let expected_cluster = Cluster {
            fee_recipient: new_fee_recipient,
            ..fixture.cluster.clone()
        };
        assertions::cluster::exists_in_db(&fixture.db, &expected_cluster);
        assertions::cluster::exists_in_memory(&fixture.db, &expected_cluster);

        // Confirm that we have set the correct fee recipient for the owner
        let stored_fee_recipient = fixture
            .db
            .fee_recipient_for_owner(&fixture.cluster.owner)
            .unwrap();
        assert_eq!(stored_fee_recipient, Some(new_fee_recipient));
    }

    #[test]
    // Test that fee_recipient_for_owner handles NULL values correctly after BUMP_NONCE
    fn test_fee_recipient_null_handling() {
        let fixture = InMemoryTestFixture::new_empty();
        let owner = Address::random();

        // Initially, the owner doesn't exist, so fee_recipient should be None
        let fee_recipient = fixture.db.fee_recipient_for_owner(&owner).unwrap();
        assert_eq!(fee_recipient, None);

        // Call BUMP_NONCE, which creates an entry with owner and nonce but NULL fee_recipient
        let nonce = fixture.db.bump_and_get_nonce(&owner).unwrap();
        assert_eq!(nonce, 0);

        // Now fee_recipient_for_owner should handle the NULL value and return None
        let fee_recipient_after_bump = fixture.db.fee_recipient_for_owner(&owner).unwrap();
        assert_eq!(fee_recipient_after_bump, None);

        // Set a fee recipient and verify it works
        let test_fee_recipient = Address::random();
        fixture
            .db
            .commit_fee_recipient_updated(owner, test_fee_recipient, test_cursor(0))
            .unwrap();

        let fee_recipient_after_update = fixture.db.fee_recipient_for_owner(&owner).unwrap();
        assert_eq!(fee_recipient_after_update, Some(test_fee_recipient));
    }

    #[test]
    // Test that nonce progression is consistent regardless of operation order
    fn test_nonce_consistency_different_operation_orders() {
        let fixture = InMemoryTestFixture::new_empty();
        let owner1 = Address::random();
        let owner2 = Address::random();

        // Scenario 1: Owner1 sets fee_recipient first, then BUMP_NONCE
        let fee_recipient1 = Address::random();
        fixture
            .db
            .commit_fee_recipient_updated(owner1, fee_recipient1, test_cursor(0))
            .unwrap();

        // Now bump the nonce
        fixture.db.bump_and_get_nonce(&owner1).unwrap();

        // Scenario 2: Owner2 calls BUMP_NONCE first, then sets fee_recipient
        fixture.db.bump_and_get_nonce(&owner2).unwrap();

        let fee_recipient2 = Address::random();
        fixture
            .db
            .commit_fee_recipient_updated(owner2, fee_recipient2, test_cursor(1))
            .unwrap();

        let nonce1 = fixture.db.get_nonce_for_owner(owner1).unwrap();
        let nonce2 = fixture.db.get_nonce_for_owner(owner2).unwrap();

        assert_eq!(
            nonce1, nonce2,
            "Nonce should be consistent regardless of operation order"
        );
    }
}
