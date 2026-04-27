#[cfg(test)]
mod operator_database_tests {
    use ssv_types::{Operator, OperatorId};

    use crate::{
        NetworkDatabase, PendingStateUpdates,
        test_utils::{
            InMemoryTestFixture, TEST_NETWORK, assertions, commit_and_publish, generators,
        },
    };

    #[test]
    // Test to make sure we can insert new operators into the database and they are present in the
    // state stores
    fn test_insert_retrieve_operator() {
        // Create a new text fixture with empty db
        let fixture = InMemoryTestFixture::new_empty();

        let mut conn = fixture.db.connection().unwrap();
        let tx = conn.transaction().unwrap();
        let mut pending = PendingStateUpdates::default();

        // Generate a new operator and insert it
        let operator = generators::operator::with_id(1);
        fixture
            .db
            .insert_operator_tx(&operator, &tx, &mut pending)
            .expect("Failed to insert operator");

        // Confirm that it exists both in the db and the state store
        assertions::operator::exists_in_db(&operator, &tx);
        commit_and_publish(&fixture.db, tx, pending);
        assertions::operator::exists_in_memory(&fixture.db, &operator);
    }

    #[test]
    fn test_insert_operator_tx_reads_own_id_before_publish() {
        let operator = generators::operator::with_id(1);
        let db = NetworkDatabase::new_in_memory(&operator.rsa_pubkey, TEST_NETWORK)
            .expect("Failed to create in-memory database");

        let mut conn = db.connection().unwrap();
        let tx = conn.transaction().unwrap();
        let mut pending = PendingStateUpdates::default();

        db.insert_operator_tx(&operator, &tx, &mut pending)
            .expect("Failed to stage operator insert");

        assert_eq!(db.state().get_own_id(), None);
        assert_eq!(
            db.get_own_operator_id_tx(&tx)
                .expect("Failed to read own operator id"),
            Some(operator.id)
        );
    }

    #[test]
    // Ensure that we cannot insert a duplicate operator into the database
    fn test_duplicate_insert() {
        // Create a new test fixture with empty db
        let fixture = InMemoryTestFixture::new_empty();

        let mut conn = fixture.db.connection().unwrap();
        let tx = conn.transaction().unwrap();
        let mut pending = PendingStateUpdates::default();

        // Generate a new operator and insert it
        let operator = generators::operator::with_id(1);
        fixture
            .db
            .insert_operator_tx(&operator, &tx, &mut pending)
            .expect("Failed to insert operator");

        // Try to insert it again, this should fail
        assert!(
            fixture
                .db
                .insert_operator_tx(&operator, &tx, &mut pending)
                .is_err()
        );
    }

    #[test]
    // Test deleting an operator and confirming it is gone from the db and in memory
    fn test_insert_delete_operator() {
        // Create new test fixture with empty db
        let fixture = InMemoryTestFixture::new_empty();

        let mut conn = fixture.db.connection().unwrap();
        let tx = conn.transaction().unwrap();
        let mut pending = PendingStateUpdates::default();

        // Generate a new operator and insert it
        let operator = generators::operator::with_id(1);
        fixture
            .db
            .insert_operator_tx(&operator, &tx, &mut pending)
            .expect("Failed to insert operator");

        // Now, delete the operator
        fixture
            .db
            .delete_operator_tx(operator.id, &tx, &mut pending)
            .expect("Failed to delete operator");

        // Confirm that it is gone
        assertions::operator::exists_not_in_db(operator.id, &tx);
        commit_and_publish(&fixture.db, tx, pending);
        assertions::operator::exists_not_in_memory(&fixture.db, operator.id);
    }

    #[test]
    // Test inserting multiple operators
    fn test_insert_multiple_operators() {
        // Create new test fixture with empty db
        let fixture = InMemoryTestFixture::new_empty();

        let mut conn = fixture.db.connection().unwrap();
        let tx = conn.transaction().unwrap();
        let mut pending = PendingStateUpdates::default();

        // Generate and insert operators
        let operators: Vec<Operator> = (0..4).map(generators::operator::with_id).collect();
        for operator in &operators {
            fixture
                .db
                .insert_operator_tx(operator, &tx, &mut pending)
                .expect("Failed to insert operator");
        }

        // Delete them all and confirm deletion
        for operator in &operators {
            fixture
                .db
                .delete_operator_tx(operator.id, &tx, &mut pending)
                .expect("Failed to delete operator");
        }
        for operator in &operators {
            assertions::operator::exists_not_in_db(operator.id, &tx);
        }
        commit_and_publish(&fixture.db, tx, pending);
        for operator in operators {
            assertions::operator::exists_not_in_memory(&fixture.db, operator.id);
        }
    }

    #[test]
    /// Try to delete an operator that does not exist
    fn test_delete_dne_operator() {
        let fixture = InMemoryTestFixture::new_empty();
        let mut conn = fixture.db.connection().unwrap();
        let tx = conn.transaction().unwrap();
        let mut pending = PendingStateUpdates::default();
        assert!(
            fixture
                .db
                .delete_operator_tx(OperatorId(1), &tx, &mut pending)
                .is_err()
        )
    }

    #[test]
    fn test_skipped_operator_add_round_trip() {
        // Arrange: start from an empty in-memory database and an operator id with no marker.
        let fixture = InMemoryTestFixture::new_empty();
        let mut conn = fixture.db.connection().unwrap();
        let tx = conn.transaction().unwrap();

        // Act/Assert: the marker should be absent before insertion.
        assert!(
            !fixture
                .db
                .was_operator_add_skipped_tx(OperatorId(7), &tx)
                .expect("Failed to query skipped operator marker")
        );

        // Act: insert a skipped-operator marker.
        fixture
            .db
            .insert_skipped_operator_add_tx(OperatorId(7), "duplicate key", &tx)
            .expect("Failed to insert skipped operator marker");

        // Assert: the inserted marker is visible through the typed helper.
        assert!(
            fixture
                .db
                .was_operator_add_skipped_tx(OperatorId(7), &tx)
                .expect("Failed to query skipped operator marker")
        );

        // Act: delete the skipped-operator marker.
        assert!(
            fixture
                .db
                .delete_skipped_operator_add_tx(OperatorId(7), &tx)
                .expect("Failed to delete skipped operator marker")
        );

        // Assert: the marker is gone again after deletion.
        assert!(
            !fixture
                .db
                .was_operator_add_skipped_tx(OperatorId(7), &tx)
                .expect("Failed to query skipped operator marker")
        );
    }
}
