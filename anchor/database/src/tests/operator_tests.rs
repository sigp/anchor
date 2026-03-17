#[cfg(test)]
mod operator_database_tests {
    use ssv_types::{Operator, OperatorId};

    use crate::test_utils::{InMemoryTestFixture, assertions, generators, test_cursor};

    #[test]
    // Test to make sure we can insert new operators into the database and they are present in the
    // state stores
    fn test_insert_retrieve_operator() {
        // Create a new text fixture with empty db
        let fixture = InMemoryTestFixture::new_empty();

        // Generate a new operator and insert it
        let operator = generators::operator::with_id(1);
        fixture
            .db
            .commit_operator_added(&operator, *operator.id, test_cursor(0))
            .expect("Failed to insert operator");

        // Confirm that it exists both in the db and the state store
        assertions::operator::exists_in_db(&fixture.db, &operator);
        assertions::operator::exists_in_memory(&fixture.db, &operator);
    }

    #[test]
    // Ensure that we cannot insert a duplicate operator into the database
    fn test_duplicate_insert() {
        // Create a new test fixture with empty db
        let fixture = InMemoryTestFixture::new_empty();

        // Generate a new operator and insert it
        let operator = generators::operator::with_id(1);
        fixture
            .db
            .commit_operator_added(&operator, *operator.id, test_cursor(0))
            .expect("Failed to insert operator");

        // Try to insert it again, this should fail
        assert!(
            fixture
                .db
                .commit_operator_added(&operator, *operator.id, test_cursor(1))
                .is_err()
        );
    }

    #[test]
    // Test deleting an operator and confirming it is gone from the db and in memory
    fn test_insert_delete_operator() {
        // Create new test fixture with empty db
        let fixture = InMemoryTestFixture::new_empty();

        // Generate a new operator and insert it
        let operator = generators::operator::with_id(1);
        fixture
            .db
            .commit_operator_added(&operator, *operator.id, test_cursor(0))
            .expect("Failed to insert operator");

        // Now, delete the operator
        fixture
            .db
            .commit_operator_removed(operator.id, test_cursor(1))
            .expect("Failed to delete operator");

        // Confirm that it is gone
        assertions::operator::exists_not_in_memory(&fixture.db, operator.id);
        assertions::operator::exists_not_in_db(&fixture.db, operator.id);
    }

    #[test]
    // Test inserting multiple operators
    fn test_insert_multiple_operators() {
        // Create new test fixture with empty db
        let fixture = InMemoryTestFixture::new_empty();

        // Generate and insert operators
        let operators: Vec<Operator> = (0..4).map(generators::operator::with_id).collect();
        for (index, operator) in operators.iter().enumerate() {
            fixture
                .db
                .commit_operator_added(operator, *operator.id, test_cursor(index as u64))
                .expect("Failed to insert operator");
        }

        // Delete them all and confirm deletion
        for (index, operator) in operators.into_iter().enumerate() {
            fixture
                .db
                .commit_operator_removed(operator.id, test_cursor((index + 4) as u64))
                .expect("Failed to delete operator");

            assertions::operator::exists_not_in_memory(&fixture.db, operator.id);
            assertions::operator::exists_not_in_db(&fixture.db, operator.id);
        }
    }

    #[test]
    /// Removing an unknown operator is a no-op at the database commit layer.
    fn test_delete_unknown_operator_is_noop() {
        let fixture = InMemoryTestFixture::new_empty();
        fixture
            .db
            .commit_operator_removed(OperatorId(1), test_cursor(0))
            .expect("Missing operators should be ignored at commit time");

        assertions::operator::exists_not_in_memory(&fixture.db, OperatorId(1));
        assertions::operator::exists_not_in_db(&fixture.db, OperatorId(1));
    }
}
