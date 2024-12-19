use super::test_prelude::*;

#[cfg(test)]
mod state_database_tests {
    use super::*;

    /*
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
            assertions::assert_operator_exists_in_db(&fixture.db, operator);
            assertions::assert_operator_exists_in_store(&fixture.db, operator);
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

        // Confirm all cluster related data is still correct
        assertions::assert_cluster_exists_in_db(&fixture.db, &cluster);
        assertions::assert_cluster_exists_in_store(&fixture.db, &cluster);
    }

    #[test]
    // Test that you can update and retrieve a block number
    fn test_block_number() {
        let fixture = TestFixture::new();
        assert_eq!(fixture.db.read_state(|state| state.last_processed_block), 0);
        fixture
            .db
            .processed_block(10)
            .expect("Failed to update the block number");
        assert_eq!(
            fixture.db.read_state(|state| state.last_processed_block),
            10
        );
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
    */
}
