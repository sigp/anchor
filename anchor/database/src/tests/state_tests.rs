#[cfg(test)]
mod state_database_tests {
    use ssv_types::{Cluster, Operator, Share, ValidatorMetadata};
    use types::Address;

    use crate::{
        NetworkDatabase, PendingStateUpdates,
        multi_index::UniqueIndex,
        test_utils::{
            DEFAULT_NUM_OPERATORS, FIRST_OPERATOR_ID, FileTestFixture, InMemoryTestFixture,
            TEST_NETWORK, assertions, commit_and_publish, generators,
        },
    };

    /// Smallest committee size above the fixture default, f=2.
    const SEVEN_OPERATOR_COMMITTEE: u64 = 7;

    /// The share held by `operator`, which the fixtures treat as this node's own operator.
    fn own_share(shares: &[Share], operator: &Operator) -> Share {
        shares
            .iter()
            .find(|share| share.operator_id == operator.id)
            .expect("This operator should hold a share")
            .clone()
    }

    /// Insert a validator into the fixture's database and publish the state update.
    fn insert_validator(
        fixture: &FileTestFixture,
        cluster: Cluster,
        validator: &ValidatorMetadata,
        shares: Vec<Share>,
    ) {
        let mut conn = fixture.data.db.connection().unwrap();
        let tx = conn.transaction().unwrap();
        let mut pending = PendingStateUpdates::default();
        fixture
            .data
            .db
            .insert_validator_tx(cluster, validator, shares, &tx, &mut pending)
            .expect("Insert should not fail");
        commit_and_publish(&fixture.data.db, tx, pending);
    }

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
            NetworkDatabase::new(&path, &pubkey, TEST_NETWORK).expect("Failed to create database");

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
            NetworkDatabase::new(&path, &pubkey, TEST_NETWORK).expect("Failed to create database");

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
            NetworkDatabase::new(&path, &pubkey, TEST_NETWORK).expect("Failed to create database");

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
        let (new_validator, shares) = generators::validator::with_consistent_shares(&cluster);
        insert_validator(&fixture, cluster, &new_validator, shares);

        // Save path and pubkey before dropping db
        let path = fixture.path.clone();
        let pubkey = fixture.pubkey.clone();

        // drop and recrate database
        drop(fixture.data.db);
        fixture.data.db =
            NetworkDatabase::new(&path, &pubkey, TEST_NETWORK).expect("Failed to create database");

        // assert that there are two validators, one cluster, and 2 shares in memory
        assert_eq!(fixture.data.db.state().metadata().length(), 2);
        assert_eq!(fixture.data.db.state().shares().length(), 2);
        assert_eq!(fixture.data.db.state().clusters().length(), 1);
    }

    // ==================== Startup share validation tests ====================

    /// The startup load must drop only the own share of a validator whose stored share public
    /// keys do not interpolate to its validator public key, while the validator metadata, the
    /// cluster and every stored share row stay intact.
    #[test]
    fn test_reopen_skips_only_shares_that_fail_interpolation() {
        // Arrange: the fixture validator has consistent shares; add a second validator in the
        // same cluster whose share public keys are unrelated to its validator key.
        let mut fixture = FileTestFixture::new();
        let cluster = fixture.cluster.clone();
        let consistent_validator = fixture.validator.clone();
        let inconsistent_validator = generators::validator::random_metadata(cluster.cluster_id);
        let inconsistent_shares: Vec<Share> = fixture
            .operators
            .iter()
            .map(|op| {
                generators::share::random(
                    cluster.cluster_id,
                    op.id,
                    &inconsistent_validator.public_key,
                )
            })
            .collect();
        insert_validator(
            &fixture,
            cluster.clone(),
            &inconsistent_validator,
            inconsistent_shares,
        );
        assert_eq!(fixture.data.db.state().shares().length(), 2);
        let own_share = own_share(&fixture.shares, &fixture.operators[0]);

        // Act: reload the state from disk.
        let path = fixture.path.clone();
        let pubkey = fixture.pubkey.clone();
        drop(fixture.data.db);
        fixture.data.db =
            NetworkDatabase::new(&path, &pubkey, TEST_NETWORK).expect("Failed to create database");

        // Assert: only the consistent validator's own share is served.
        let db = &fixture.data.db;
        assert_eq!(db.state().shares().length(), 1);
        assertions::share::exists_in_memory(db, &consistent_validator.public_key, &own_share);
        assertions::share::exists_not_in_memory(db, &inconsistent_validator.public_key);

        // Metadata and cluster membership are untouched by the share check.
        assertions::validator::exists_in_memory(db, &consistent_validator);
        assertions::validator::exists_in_memory(db, &inconsistent_validator);
        assertions::cluster::exists_in_memory(db, &cluster);

        // The share rows for both validators are still stored.
        for validator in [&consistent_validator, &inconsistent_validator] {
            let stored = db
                .get_share_pubkeys_for_validator(&validator.public_key)
                .expect("Share rows should be readable");
            assert_eq!(stored.len(), DEFAULT_NUM_OPERATORS as usize);
        }
    }

    /// A seven operator committee has quorum five, so the startup check interpolates a degree
    /// four polynomial. Consistent shares for such a cluster must survive a reopen.
    #[test]
    fn test_reopen_keeps_consistent_shares_in_seven_operator_cluster() {
        // Arrange: extend the default operator set to seven, including this operator, and
        // register a validator with shares split over all seven.
        let mut fixture = FileTestFixture::new();
        let extra_operators: Vec<Operator> = (FIRST_OPERATOR_ID + DEFAULT_NUM_OPERATORS
            ..FIRST_OPERATOR_ID + SEVEN_OPERATOR_COMMITTEE)
            .map(generators::operator::with_id)
            .collect();
        {
            let mut conn = fixture.data.db.connection().unwrap();
            let tx = conn.transaction().unwrap();
            let mut pending = PendingStateUpdates::default();
            for operator in &extra_operators {
                fixture
                    .data
                    .db
                    .insert_operator_tx(operator, &tx, &mut pending)
                    .expect("Failed to insert operator");
            }
            commit_and_publish(&fixture.data.db, tx, pending);
        }
        let mut committee = fixture.operators.clone();
        committee.extend(extra_operators);
        assert_eq!(committee.len(), SEVEN_OPERATOR_COMMITTEE as usize);
        let cluster = generators::cluster::with_operators(&committee);
        let (validator, shares) = generators::validator::with_consistent_shares(&cluster);
        let own_share = own_share(&shares, &fixture.operators[0]);
        insert_validator(&fixture, cluster.clone(), &validator, shares);

        // Act: reload the state from disk.
        let path = fixture.path.clone();
        let pubkey = fixture.pubkey.clone();
        drop(fixture.data.db);
        fixture.data.db =
            NetworkDatabase::new(&path, &pubkey, TEST_NETWORK).expect("Failed to create database");

        // Assert: the share for the seven operator validator is served alongside the default
        // fixture validator's share.
        let db = &fixture.data.db;
        assert_eq!(db.state().shares().length(), 2);
        assertions::share::exists_in_memory(db, &validator.public_key, &own_share);
        assertions::validator::exists_in_memory(db, &validator);
        assertions::cluster::exists_in_memory(db, &cluster);
    }

    #[test]
    // Test that you can update and retrieve a block number
    fn test_block_number() {
        let fixture = InMemoryTestFixture::new();
        assert_eq!(fixture.db.state().get_last_processed_block(), 0);
        let mut conn = fixture.db.connection().unwrap();
        let tx = conn.transaction().unwrap();
        let mut pending = PendingStateUpdates::default();
        fixture
            .db
            .processed_block_tx(10, &tx, &mut pending)
            .expect("Failed to update the block number");
        commit_and_publish(&fixture.db, tx, pending);

        assert_eq!(fixture.db.state().get_last_processed_block(), 10);
    }

    #[test]
    // Test to make sure the block number is loaded in after restart
    fn test_block_number_after_restart() {
        let mut fixture = FileTestFixture::new();
        let mut conn = fixture.data.db.connection().unwrap();
        let tx = conn.transaction().unwrap();
        let mut pending = PendingStateUpdates::default();
        fixture
            .data
            .db
            .processed_block_tx(10, &tx, &mut pending)
            .expect("Failed to update the block number");
        commit_and_publish(&fixture.data.db, tx, pending);

        // Save path and pubkey before dropping db
        let path = fixture.path.clone();
        let pubkey = fixture.pubkey.clone();

        drop(fixture.data.db);
        drop(conn);

        fixture.data.db =
            NetworkDatabase::new(&path, &pubkey, TEST_NETWORK).expect("Failed to create database");
        assert_eq!(fixture.data.db.state().get_last_processed_block(), 10);
    }

    #[test]
    fn test_processed_block_tx_defers_publication_until_explicit_publish() {
        let fixture = InMemoryTestFixture::new_empty();
        let mut conn = fixture.db.connection().unwrap();
        let tx = conn.transaction().unwrap();
        let mut pending = PendingStateUpdates::default();

        fixture
            .db
            .processed_block_tx(42, &tx, &mut pending)
            .expect("Failed to stage block number");

        let staged_block: u64 = tx
            .query_row("SELECT block_number FROM metadata", [], |row| row.get(0))
            .expect("Block number should be visible in the active transaction");
        assert_eq!(staged_block, 42);
        assert_eq!(fixture.db.state().get_last_processed_block(), 0);

        tx.commit().unwrap();
        assert_eq!(fixture.db.state().get_last_processed_block(), 0);

        fixture.db.publish_pending_state_updates(pending);
        assert_eq!(fixture.db.state().get_last_processed_block(), 42);
    }

    #[test]
    // Test to make sure we can retrieve and increment a nonce
    fn test_retrieve_increment_nonce() {
        let fixture = InMemoryTestFixture::new();
        let owner = Address::random();
        let mut conn = fixture.db.connection().unwrap();
        let tx = conn.transaction().unwrap();
        let mut pending = PendingStateUpdates::default();

        // this is the first time getting the nonce, so it should be zero
        let nonce = fixture
            .db
            .bump_and_get_nonce_tx(&owner, &tx, &mut pending)
            .expect("Failed in increment nonce");
        assert_eq!(nonce, 0);

        // increment the nonce and then confirm that is is one
        let nonce = fixture
            .db
            .bump_and_get_nonce_tx(&owner, &tx, &mut pending)
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
        let mut pending = PendingStateUpdates::default();
        fixture
            .data
            .db
            .bump_and_get_nonce_tx(&owner, &tx, &mut pending)
            .expect("Failed in increment nonce");
        commit_and_publish(&fixture.data.db, tx, pending);

        // Save path and pubkey before dropping db
        let path = fixture.path.clone();
        let pubkey = fixture.pubkey.clone();

        drop(conn);
        drop(fixture.data.db);
        fixture.data.db =
            NetworkDatabase::new(&path, &pubkey, TEST_NETWORK).expect("Failed to create database");
        let mut conn = fixture.data.db.connection().unwrap();
        let tx = conn.transaction().unwrap();
        let mut pending = PendingStateUpdates::default();

        // confirm that nonce is 1
        assert_eq!(
            fixture
                .data
                .db
                .bump_and_get_nonce_tx(&owner, &tx, &mut pending)
                .expect("Failed in increment nonce"),
            1
        );
    }
}
