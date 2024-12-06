use super::{DatabaseError, NetworkDatabase, SqlStatement, SQL};
use rsa::pkcs8::{EncodePublicKey, LineEnding};
use rsa::RsaPublicKey;
use rusqlite::params;
use ssv_types::{Operator, OperatorId};

/// Implements all operator related functionality on the database
impl NetworkDatabase {
    /// Insert a new operator into the database
    pub fn insert_operator(&mut self, operator: &Operator) -> Result<(), DatabaseError> {
        // make sure that this operator does not already exist
        if self.operators.contains_key(&operator.id) {
            return Err(DatabaseError::NotFound(format!(
                "Operator with id {} not in database",
                *operator.id
            )));
        }

        let conn = self.connection()?;
        conn.prepare_cached(SQL[&SqlStatement::InsertOperator])?
            .execute(params![
                *operator.id,
                Self::encode_pubkey(&operator.rsa_pubkey),
                operator.owner.to_string()
            ])?;

        // then, store in memory
        self.operators.insert(operator.id, operator.clone());
        Ok(())
    }

    /// Delete an operator
    pub fn delete_operator(&mut self, id: OperatorId) -> Result<(), DatabaseError> {
        // make sure that it exists
        if !self.operators.contains_key(&id) {
            return Err(DatabaseError::NotFound(format!(
                "Operator with id {} not in database",
                *id
            )));
        }

        // Remove from db and in memory. This should cascade to delete this operator from all of the
        // clusters that it is in and all of the shares that it owns
        let conn = self.connection()?;
        conn.prepare_cached(SQL[&SqlStatement::DeleteOperator])?
            .execute(params![*id])?;
        self.operators.remove(&id);
        Ok(())
    }

    /// Get operator data from in memory store
    pub fn get_operator(&self, id: &OperatorId) -> Option<&Operator> {
        self.operators.get(id)
    }

    /// Check to see if the operator exists
    pub fn operator_exists(&self, id: &OperatorId) -> bool {
        self.operators.contains_key(id)
    }

    // Helper to encode the RsaPublicKey to PEM string
    fn encode_pubkey(pubkey: &RsaPublicKey) -> String {
        // this should never fail as the key has already been validated upon construction
        pubkey
            .to_public_key_pem(LineEnding::default())
            .expect("Failed to encode RsaPublicKey")
    }
}

#[cfg(test)]
mod operator_database_tests {
    use super::*;
    use crate::test_utils::{dummy_operator, get_operator_from_db};
    use tempfile::tempdir;

    #[test]
    // Test inserting into the database and then confirming that it is both in
    // memory and in the underlying database
    fn test_insert_retrieve_operator() {
        // Create a temporary database
        let dir = tempdir().unwrap();
        let file = dir.path().join("db.sqlite");
        let mut db = NetworkDatabase::create(&file).unwrap();

        // Insert dummy operator data into the database
        let operator = dummy_operator(1);
        assert!(db.insert_operator(&operator).is_ok());

        // Fetch operator from in memory store and confirm values
        let fetched_operator = db.get_operator(&operator.id);
        if let Some(op) = fetched_operator {
            assert_eq!(op.id, operator.id);
            assert_eq!(op.rsa_pubkey, operator.rsa_pubkey);
            assert_eq!(op.owner, operator.owner);
        } else {
            panic!("Expected to find operator in memory");
        }

        // Check to make sure the operator is also in the underlying db
        let db_operator = get_operator_from_db(&db, operator.id);
        if let Some(op) = db_operator {
            assert_eq!(op.rsa_pubkey, operator.rsa_pubkey);
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
        let mut db = NetworkDatabase::create(&file).unwrap();

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
        let mut db = NetworkDatabase::create(&file).unwrap();

        for id in 0..4 {
            let operator = dummy_operator(id);
            assert!(db.insert_operator(&operator).is_ok());
        }
    }
}
