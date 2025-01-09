use super::test_prelude::*;

#[cfg(test)]
mod operator_database_tests {
    use super::*;

    #[test]
    // Test to make sure we can insert new operators into the database and they are present in the
    // state stores
    fn test_insert_retrieve_operator() {
        // Create a new text fixture with empty db
        let fixture = TestFixture::new_empty();

        // Generate a new operator and insert it
        let operator = generators::operator::with_id(1);
        fixture
            .db
            .insert_operator(&operator)
            .expect("Failed to insert operator");

        // Confirm that it exists both in the db and the state store
        assertions::operator::exists_in_db(&fixture.db, &operator);
        assertions::operator::exists_in_memory(&fixture.db, &operator);
    }

    #[test]
    // Ensure that we cannot insert a duplicate operator into the database
    fn test_duplicate_insert() {
        // Create a new test fixture with empty db
        let fixture = TestFixture::new_empty();

        // Generate a new operator and insert it
        let operator = generators::operator::with_id(1);
        fixture
            .db
            .insert_operator(&operator)
            .expect("Failed to insert operator");

        // Try to insert it again, this should fail
        assert!(fixture.db.insert_operator(&operator).is_err());
    }

    #[test]
    // Test deleting an operator and confirming it is gone from the db and in memory
    fn test_insert_delete_operator() {
        // Create new test fixture with empty db
        let fixture = TestFixture::new_empty();

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
        assertions::operator::exists_not_in_memory(&fixture.db, operator.id);
        assertions::operator::exists_not_in_db(&fixture.db, operator.id);
    }

    #[test]
    // Test inserting multiple operators
    fn test_insert_multiple_operators() {
        // Create new test fixture with empty db
        let fixture = TestFixture::new_empty();

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
            assertions::operator::exists_not_in_memory(&fixture.db, operator.id);
            assertions::operator::exists_not_in_db(&fixture.db, operator.id);
        }
    }

    #[test]
    /// Try to delete an operator that does not exist
    fn test_delete_dne_operator() {
        let fixture = TestFixture::new_empty();
        assert!(fixture.db.delete_operator(OperatorId(1)).is_err())
    }
}
