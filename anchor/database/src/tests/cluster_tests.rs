use super::test_prelude::*;

#[cfg(test)]
mod cluster_database_tests {
    use super::*;

    #[test]
    // Test inserting a cluster into the database
    fn test_insert_retrieve_cluster() {
        let fixture = TestFixture::new();

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
        let fixture = TestFixture::new();
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
        let fixture = TestFixture::new();
        let mut cluster = fixture.cluster;
        let new_fee_recipient = Address::random();

        let mut conn = fixture.db.connection().unwrap();
        let tx = conn.transaction().unwrap();

        // Update fee recipient
        assert!(
            fixture
                .db
                .update_fee_recipient(cluster.owner, new_fee_recipient, &tx)
                .is_ok()
        );

        // assertions will compare the data
        cluster.fee_recipient = new_fee_recipient;
        assertions::cluster::exists_in_db(&cluster, &tx);
        assertions::cluster::exists_in_memory(&fixture.db, &cluster);
    }

    #[test]
    // Try inserting a cluster that does not already have registers operators in the database
    fn test_insert_cluster_without_operators() {
        let fixture = TestFixture::new_empty();
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
        let fixture = TestFixture::new();
        let mut cluster = fixture.cluster;

        let mut conn = fixture.db.connection().unwrap();
        let tx = conn.transaction().unwrap();

        // Test updating to liquidated
        fixture
            .db
            .update_status(cluster.cluster_id, true, &tx)
            .expect("Failed to update cluster status");

        // verify in memory and db
        cluster.liquidated = true;
        assertions::cluster::exists_in_db(&cluster, &tx);
        assertions::cluster::exists_in_memory(&fixture.db, &cluster);
    }

    #[test]
    // Test inserting a cluster that already exists
    fn test_duplicate_cluster_insert() {
        let fixture = TestFixture::new();
        let mut conn = fixture.db.connection().unwrap();
        let tx = conn.transaction().unwrap();
        fixture
            .db
            .insert_validator(fixture.cluster, &fixture.validator, fixture.shares, &tx)
            .expect_err("Expected failure when inserting cluster that already exists");
    }

    #[test]
    // Test that we can properly track the fee recipient for an owner
    fn test_fetch_fee_recipient() {
        let fixture = TestFixture::new();
        let mut cluster = fixture.cluster;

        let mut conn = fixture.db.connection().unwrap();
        let tx = conn.transaction().unwrap();

        // Confirm that the fee recipient was inserted when the cluster was made
        let fee_recipient = fixture
            .db
            .fee_recipient_for_owner(&cluster.owner, &tx)
            .unwrap();
        assert_eq!(fee_recipient, Some(cluster.fee_recipient));

        // Update fee recipient
        let new_fee_recipient = Address::random();
        assert!(
            fixture
                .db
                .update_fee_recipient(cluster.owner, new_fee_recipient, &tx)
                .is_ok()
        );

        // Confirm that fee recipient was updated
        cluster.fee_recipient = new_fee_recipient;
        assertions::cluster::exists_in_db(&cluster, &tx);
        assertions::cluster::exists_in_memory(&fixture.db, &cluster);

        // Confirm that we have set the correct fee recipient for the owner
        let stored_fee_recipient = fixture
            .db
            .fee_recipient_for_owner(&cluster.owner, &tx)
            .unwrap();
        assert_eq!(stored_fee_recipient, Some(new_fee_recipient));
    }
}
