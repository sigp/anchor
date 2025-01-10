use super::test_prelude::*;

#[cfg(test)]
mod state_database_tests {
    use super::*;

    #[test]
    // Test that the previously inserted operators are present after restart
    fn test_operator_store() {
        // Create new test fixture with populated DB
        let mut fixture = TestFixture::new();

        // drop the database and then recreate it
        drop(fixture.db);
        fixture.db = NetworkDatabase::new(&fixture.path, &fixture.pubkey)
            .expect("Failed to create database");

        // confirm that all of the operators exist
        for operator in &fixture.operators {
            assertions::operator::exists_in_db(&fixture.db, operator);
            assertions::operator::exists_in_memory(&fixture.db, operator);
        }
    }

    #[test]
    // Test that the proper cluster data is present after restart
    fn test_cluster_after_restart() {
        // Create new test fixture with populated DB
        let mut fixture = TestFixture::new();
        let cluster = fixture.cluster;

        // drop the database and then recreate it
        drop(fixture.db);
        fixture.db = NetworkDatabase::new(&fixture.path, &fixture.pubkey)
            .expect("Failed to create database");

        // confirm all data is what we expect
        assertions::cluster::exists_in_memory(&fixture.db, &cluster);
        assertions::validator::exists_in_memory(&fixture.db, &fixture.validator);
    }

    #[test]
    // Test that a this operator owns is in memory after restart
    fn test_shares_after_restart() {
        // Create new test fixture with populated DB
        let mut fixture = TestFixture::new();

        // drop and recrate database
        drop(fixture.db);
        fixture.db = NetworkDatabase::new(&fixture.path, &fixture.pubkey)
            .expect("Failed to create database");

        // Confim share data, there should be one share in memory for this operator
        assert!(fixture.db.shares().length() == 1);
        let pk = &fixture.validator.public_key;
        let share = fixture
            .db
            .shares()
            .get_by(pk)
            .expect("The share should exist");
        assertions::share::exists_in_memory(&fixture.db, pk, &share);
    }

    #[test]
    // Test that we have multi validators in memory after restart
    fn test_multiple_entries() {
        // Create new test fixture with populated DB
        let mut fixture = TestFixture::new();

        // Generate new validator information
        let cluster = fixture.cluster;
        let new_validator = generators::validator::random_metadata(cluster.cluster_id);
        let mut shares: Vec<Share> = Vec::new();
        fixture.operators.iter().for_each(|op| {
            let share =
                generators::share::random(cluster.cluster_id, op.id, &new_validator.public_key);
            shares.push(share);
        });
        fixture
            .db
            .insert_validator(cluster, new_validator, shares)
            .expect("Insert should not fail");

        // drop and recrate database
        drop(fixture.db);
        fixture.db = NetworkDatabase::new(&fixture.path, &fixture.pubkey)
            .expect("Failed to create database");

        // assert that there are two validators, one cluster, and 2 shares in memory
        assert!(fixture.db.metadata().length() == 2);
        assert!(fixture.db.shares().length() == 2);
        assert!(fixture.db.clusters().length() == 1);
    }

    #[test]
    // Test that you can update and retrieve a block number
    fn test_block_number() {
        let fixture = TestFixture::new();
        assert_eq!(fixture.db.get_last_processed_block(), 0);
        fixture
            .db
            .processed_block(10)
            .expect("Failed to update the block number");
        assert_eq!(fixture.db.get_last_processed_block(), 10);
    }

    #[test]
    // Test to make sure the block number is loaded in after restart
    fn test_block_number_after_restart() {
        let mut fixture = TestFixture::new();
        fixture
            .db
            .processed_block(10)
            .expect("Failed to update the block number");
        drop(fixture.db);

        fixture.db = NetworkDatabase::new(&fixture.path, &fixture.pubkey)
            .expect("Failed to create database");
        assert_eq!(fixture.db.get_last_processed_block(), 10);
    }

    #[test]
    // Test to make sure we can retrieve and increment a nonce
    fn test_retrieve_increment_nonce() {
        let fixture = TestFixture::new();
        let owner = Address::random();

        // this is the first time getting the nonce, so it should be zero
        let nonce = fixture.db.get_nonce(&owner);
        assert_eq!(nonce, 0);

        // increment the nonce and then confirm that is is one
        fixture
            .db
            .bump_nonce(&owner)
            .expect("Failed in increment nonce");
        let nonce = fixture.db.get_nonce(&owner);
        assert_eq!(nonce, 1);
    }

    #[test]
    // Test to make sure a nonce persists after a restart
    fn test_nonce_after_restart() {
        let mut fixture = TestFixture::new();
        let owner = Address::random();
        fixture
            .db
            .bump_nonce(&owner)
            .expect("Failed in increment nonce");

        drop(fixture.db);
        fixture.db = NetworkDatabase::new(&fixture.path, &fixture.pubkey)
            .expect("Failed to create database");

        // confirm that nonce is 1
        assert_eq!(fixture.db.get_nonce(&owner), 1);
    }
}
