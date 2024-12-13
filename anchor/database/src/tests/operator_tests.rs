use super::test_prelude::*;

#[cfg(test)]
mod operator_database_tests {
    use super::*;

    #[test]
    // Test to make sure we can insert new operators into the database and they are present in the
    // state stores
    fn test_insert_retrieve_operator() {
        // Create a new text fixture with empty db
        let mut fixture = TestFixture::new_empty();

        // Generate a new operator and insert it
        let operator = generators::operator::with_id(1);
        fixture
            .db
            .insert_operator(&operator)
            .expect("Failed to insert operator");

        // Confirm that it exists both in the db and the state store
        assertions::assert_operator_exists_in_db(&fixture.db, &operator);
        assertions::assert_operator_exists_in_store(&fixture.db, &operator);
    }

    #[test]
    // Ensure that we cannot insert a duplicate operator into the database
    fn test_duplicate_insert() {
        // Create a new test fixture with empty db
        let mut fixture = TestFixture::new_empty();

        // Generate a new operator and insert it
        let operator = generators::operator::with_id(1);
        fixture
            .db
            .insert_operator(&operator)
            .expect("Failed to insert operator");

        // Try to insert it again, this should fail
        let success = fixture.db.insert_operator(&operator);
        if success.is_ok() {
            panic!("Expected an error when inserting an operator that is already present");
        }
    }

    #[test]
    // Test deleting an operator and confirming it is gone from the db and in memory
    fn test_insert_delete_operator() {
        // Create new test fixture with empty db
        let mut fixture = TestFixture::new_empty();

        // Generate a new operator and insert it
        let operator = generators::operator::with_id(1);
        fixture
            .db
            .insert_operator(&operator)
            .expect("Failed to insert operator");

        // Now, delete the operator
        fixture
            .db
            .delete_operator(operator.id)
            .expect("Failed to delete operator");

        // Confirm that it is gone
        assertions::assert_operator_not_exists_in_db(&fixture.db, operator.id);
        assertions::assert_operator_not_exists_in_store(&fixture.db, operator.id);
    }

    #[test]
    // Test inserting multiple operators
    fn test_insert_multiple_operators() {
        // Create new test fixture with empty db
        let mut fixture = TestFixture::new_empty();

        // Generate and insert operators
        let operators: Vec<Operator> = (0..4).map(generators::operator::with_id).collect();
        for operator in &operators {
            fixture
                .db
                .insert_operator(operator)
                .expect("Failed to insert operator");
        }

        // Delete them all and confirm deletion
        for operator in operators {
            fixture
                .db
                .delete_operator(operator.id)
                .expect("Failed to delete operator");
            assertions::assert_operator_not_exists_in_db(&fixture.db, operator.id);
            assertions::assert_operator_not_exists_in_store(&fixture.db, operator.id);
        }
    }

    #[test]
    /// Try to delete an operator that does not exist
    fn test_delete_dne_operator() {
        let mut fixture = TestFixture::new_empty();
        fixture
            .db
            .delete_operator(OperatorId(1))
            .expect_err("Deletion should fail. Operator DNE");
    }
}
