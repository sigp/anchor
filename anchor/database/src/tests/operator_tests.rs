use super::test_prelude::*;

#[cfg(test)]
mod operator_database_tests {
    use super::*;

    #[test]
    // Test inserting into the database and then confirming that it is both in
    // memory and in the underlying database
    fn test_insert_retrieve_operator() {
        // Create a temporary database
        let dir = tempdir().unwrap();
        let file = dir.path().join("db.sqlite");
        let mut db = NetworkDatabase::new(&file, None).unwrap();

        // Insert dummy operator data into the database
        let operator = dummy_operator(1);
        assert!(db.insert_operator(&operator).is_ok());

        // Fetch operator from in memory store and confirm values
        let fetched_operator = db.get_operator(&operator.id);
        if let Some(op) = fetched_operator {
            assert_eq!(op.id, operator.id);

            assert_eq!(
                op.rsa_pubkey.public_key_to_pem().unwrap(),
                operator.rsa_pubkey.public_key_to_pem().unwrap()
            );
            assert_eq!(op.owner, operator.owner);
        } else {
            panic!("Expected to find operator in memory");
        }

        // Check to make sure the operator is also in the underlying db
        let db_operator = get_operator_from_db(&db, operator.id);
        if let Some(op) = db_operator {
            assert_eq!(
                op.rsa_pubkey.public_key_to_pem().unwrap(),
                operator.rsa_pubkey.public_key_to_pem().unwrap()
            );
            assert_eq!(op.id, operator.id);
            assert_eq!(op.owner, operator.owner);
        } else {
            panic!("Expected to find operator in database");
        }
    }

    #[test]
    // Test deleting an operator and confirming it is gone from the db and in memory
    fn test_insert_delete_operator() {
        // Create a temporary database
        let dir = tempdir().unwrap();
        let file = dir.path().join("db.sqlite");
        let mut db = NetworkDatabase::new(&file, None).unwrap();

        // Insert dummy operator data into the database
        let operator = dummy_operator(1);
        let _ = db.insert_operator(&operator);

        // Now, delete the operator
        assert!(db.delete_operator(operator.id).is_ok());

        // Confirm that is it removed from in memory
        assert!(db.get_operator(&operator.id).is_none());

        // Also confirm that it is removed from the database
        assert!(get_operator_from_db(&db, operator.id).is_none());
    }

    #[test]
    // insert multiple operators
    fn test_insert_multiple_operators() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("db.sqlite");
        let mut db = NetworkDatabase::new(&file, None).unwrap();

        for id in 0..4 {
            let operator = dummy_operator(id);
            assert!(db.insert_operator(&operator).is_ok());
        }
    }
}
