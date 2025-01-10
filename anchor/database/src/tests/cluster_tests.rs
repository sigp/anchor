use super::test_prelude::*;

#[cfg(test)]
mod cluster_database_tests {
    use super::*;

    #[test]
    // Test inserting a cluster into the database
    fn test_insert_retrieve_cluster() {
        let fixture = TestFixture::new();
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
        let fixture = TestFixture::new();
        let pubkey = fixture.validator.public_key.clone();
        assert!(fixture.db.delete_validator(&pubkey).is_ok());

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
        let fixture = TestFixture::new();
        let mut cluster = fixture.cluster;
        let new_fee_recipient = Address::random();

        // Update fee recipient
        assert!(fixture
            .db
            .update_fee_recipient(cluster.owner, new_fee_recipient)
            .is_ok());

        //assertions will compare the data
        cluster.fee_recipient = new_fee_recipient;
        assertions::cluster::exists_in_db(&fixture.db, &cluster);
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
        fixture
            .db
            .insert_validator(cluster, metadata, shares)
            .expect_err("Insertion should fail");
    }

    #[test]
    // Test updating the operational status of the cluster
    fn test_update_cluster_status() {
        let fixture = TestFixture::new();
        let mut cluster = fixture.cluster;

        // Test updating to liquidated
        fixture
            .db
            .update_status(cluster.cluster_id, true)
            .expect("Failed to update cluster status");

        // verify in memory and db
        cluster.liquidated = true;
        assertions::cluster::exists_in_db(&fixture.db, &cluster);
        assertions::cluster::exists_in_memory(&fixture.db, &cluster);
    }

    #[test]
    // Test inserting a cluster that already exists
    fn test_duplicate_cluster_insert() {
        let fixture = TestFixture::new();
        fixture
            .db
            .insert_validator(fixture.cluster, fixture.validator, fixture.shares)
            .expect_err("Expected failure when inserting cluster that already exists");
    }
}
