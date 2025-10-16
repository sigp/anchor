#[cfg(test)]
mod state_database_tests {
    use ssv_types::Share;
    use types::Address;

    use crate::{
        NetworkDatabase,
        multi_index::UniqueIndex,
        test_utils::{FileTestFixture, InMemoryTestFixture, TEST_DOMAIN, assertions, generators},
    };

    #[test]
    // Test that the previously inserted operators are present after restart
    fn test_operator_store() {
        // Create new test fixture with populated DB - use file-based for persistence
        let mut fixture = FileTestFixture::new();

        // Save path and pubkey before dropping db
        let path = fixture.path.clone();
        let pubkey = fixture.pubkey.clone();

        // drop the database and then recreate it
        drop(fixture.data.db);
        fixture.data.db =
            NetworkDatabase::new(&path, &pubkey, TEST_DOMAIN).expect("Failed to create database");

        let mut conn = fixture.data.db.connection().unwrap();
        let tx = conn.transaction().unwrap();

        // confirm that all of the operators exist
        for operator in &fixture.operators {
            assertions::operator::exists_in_db(operator, &tx);
            assertions::operator::exists_in_memory(&fixture.data.db, operator);
        }
    }

    #[test]
    // Test that the proper cluster data is present after restart
    fn test_cluster_after_restart() {
        // Create new test fixture with populated DB - use file-based for persistence
        let mut fixture = FileTestFixture::new();

        // Save path and pubkey before dropping db
        let path = fixture.path.clone();
        let pubkey = fixture.pubkey.clone();

        // drop the database and then recreate it
        drop(fixture.data.db);
        fixture.data.db =
            NetworkDatabase::new(&path, &pubkey, TEST_DOMAIN).expect("Failed to create database");

        // confirm all data is what we expect
        assertions::cluster::exists_in_memory(&fixture.data.db, &fixture.cluster);
        assertions::validator::exists_in_memory(&fixture.data.db, &fixture.validator);
    }

    #[test]
    // Test that a this operator owns is in memory after restart
    fn test_shares_after_restart() {
        // Create new test fixture with populated DB - use file-based for persistence
        let mut fixture = FileTestFixture::new();

        // Save path and pubkey before dropping db
        let path = fixture.path.clone();
        let pubkey = fixture.pubkey.clone();

        // drop and recrate database
        drop(fixture.data.db);
        fixture.data.db =
            NetworkDatabase::new(&path, &pubkey, TEST_DOMAIN).expect("Failed to create database");

        // Confirm share data, there should be one share in memory for this operator
        assert_eq!(fixture.data.db.state().shares().length(), 1);
        let pk = &fixture.validator.public_key;
        let state = fixture.data.db.state();
        let share = state.shares().get_by(pk).expect("The share should exist");
        assertions::share::exists_in_memory(&fixture.data.db, pk, share);
    }

    #[test]
    // Test that we have multi validators in memory after restart
    fn test_multiple_entries() {
        // Create new test fixture with populated DB - use file-based for persistence
        let mut fixture = FileTestFixture::new();

        // Generate new validator information
        let cluster = fixture.cluster.clone();
        let new_validator = generators::validator::random_metadata(cluster.cluster_id);
        let mut shares: Vec<Share> = Vec::new();
        fixture.operators.iter().for_each(|op| {
            let share =
                generators::share::random(cluster.cluster_id, op.id, &new_validator.public_key);
            shares.push(share);
        });
        let mut conn = fixture.data.db.connection().unwrap();
        let tx = conn.transaction().unwrap();
        fixture
            .data
            .db
            .insert_validator(cluster, &new_validator, shares, &tx)
            .expect("Insert should not fail");
        tx.commit().unwrap();

        // Save path and pubkey before dropping db
        let path = fixture.path.clone();
        let pubkey = fixture.pubkey.clone();

        // drop and recrate database
        drop(fixture.data.db);
        drop(conn);
        fixture.data.db =
            NetworkDatabase::new(&path, &pubkey, TEST_DOMAIN).expect("Failed to create database");

        // assert that there are two validators, one cluster, and 2 shares in memory
        assert_eq!(fixture.data.db.state().metadata().length(), 2);
        assert_eq!(fixture.data.db.state().shares().length(), 2);
        assert_eq!(fixture.data.db.state().clusters().length(), 1);
    }

    #[test]
    // Test that you can update and retrieve a block number
    fn test_block_number() {
        let fixture = InMemoryTestFixture::new();
        assert_eq!(fixture.db.state().get_last_processed_block(), 0);
        let mut conn = fixture.db.connection().unwrap();
        let tx = conn.transaction().unwrap();
        fixture
            .db
            .processed_block(10, &tx)
            .expect("Failed to update the block number");
        tx.commit().unwrap();

        assert_eq!(fixture.db.state().get_last_processed_block(), 10);
    }

    #[test]
    // Test to make sure the block number is loaded in after restart
    fn test_block_number_after_restart() {
        let mut fixture = FileTestFixture::new();
        let mut conn = fixture.data.db.connection().unwrap();
        let tx = conn.transaction().unwrap();
        fixture
            .data
            .db
            .processed_block(10, &tx)
            .expect("Failed to update the block number");
        tx.commit().unwrap();

        // Save path and pubkey before dropping db
        let path = fixture.path.clone();
        let pubkey = fixture.pubkey.clone();

        drop(fixture.data.db);
        drop(conn);

        fixture.data.db =
            NetworkDatabase::new(&path, &pubkey, TEST_DOMAIN).expect("Failed to create database");
        assert_eq!(fixture.data.db.state().get_last_processed_block(), 10);
    }

    #[test]
    // Test to make sure we can retrieve and increment a nonce
    fn test_retrieve_increment_nonce() {
        let fixture = InMemoryTestFixture::new();
        let owner = Address::random();
        let mut conn = fixture.db.connection().unwrap();
        let tx = conn.transaction().unwrap();

        // this is the first time getting the nonce, so it should be zero
        let nonce = fixture
            .db
            .bump_and_get_nonce(&owner, &tx)
            .expect("Failed in increment nonce");
        assert_eq!(nonce, 0);

        // increment the nonce and then confirm that is is one
        let nonce = fixture
            .db
            .bump_and_get_nonce(&owner, &tx)
            .expect("Failed in increment nonce");
        assert_eq!(nonce, 1);
    }

    #[test]
    // Test to make sure a nonce persists after a restart
    fn test_nonce_after_restart() {
        let mut fixture = FileTestFixture::new();
        let owner = Address::random();
        let mut conn = fixture.data.db.connection().unwrap();

        let tx = conn.transaction().unwrap();
        fixture
            .data
            .db
            .bump_and_get_nonce(&owner, &tx)
            .expect("Failed in increment nonce");

        tx.commit().unwrap();

        // Save path and pubkey before dropping db
        let path = fixture.path.clone();
        let pubkey = fixture.pubkey.clone();

        drop(conn);
        drop(fixture.data.db);
        fixture.data.db =
            NetworkDatabase::new(&path, &pubkey, TEST_DOMAIN).expect("Failed to create database");
        let mut conn = fixture.data.db.connection().unwrap();
        let tx = conn.transaction().unwrap();

        // confirm that nonce is 1
        assert_eq!(
            fixture
                .data
                .db
                .bump_and_get_nonce(&owner, &tx)
                .expect("Failed in increment nonce"),
            1
        );
    }
}
