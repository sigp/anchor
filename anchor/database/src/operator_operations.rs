use super::NetworkDatabase;
use rsa::pkcs8::{EncodePublicKey, LineEnding};
use rsa::RsaPublicKey;
use rusqlite::params;
use ssv_types::{Operator, OperatorId};

/// Implements all operator related functionality on the database
impl NetworkDatabase {
    /// Insert a new operator into the database
    pub fn insert_operator(&mut self, operator: &Operator) -> Result<(), String> {
        let conn = self.connection()?;

        // encode data and insert into database
        let encoded_pubkey = Self::encode_pubkey(&operator.rsa_pubkey);
        let converted_address = operator.owner.to_string();
        conn.execute(
            "INSERT INTO operators (operator_id, public_key, owner_address) VALUES (?1, ?2, ?3)",
            params![*operator.id, encoded_pubkey, converted_address], // Note: I also fixed the parameter order to match the columns
        )
        .map_err(|e| format!("Failed to insert operator: {:?}", e))?; // Better error handling

        // then, store in memory
        self.operators.insert(operator.id, operator.clone());
        Ok(())
    }

    /// Delete an operator
    pub fn delete_operator(&mut self, id: OperatorId) -> Result<(), String> {
        // make sure that it exists
        if !self.operators.contains_key(&id) {
            return Ok(());
        }

        // Remove from db and in memory
        let conn = self.connection()?;
        conn.execute("DELETE FROM operators WHERE operator_id = ?1", params![*id])
            .map_err(|e| format!("Failed to delete operator: {:?}", e))?;
        self.operators.remove(&id);
        Ok(())
    }

    /// Get operator data from in memory store
    pub fn get_operator(&self, id: &OperatorId) -> Option<Operator> {
        self.operators.get(id).cloned()
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
    use rsa::RsaPrivateKey;
    use tempfile::tempdir;
    use types::Address;

    // Generate random operator data
    fn dummy_operator() -> Operator {
        let op_id = OperatorId(10);
        let address = Address::random();
        let _priv_key = RsaPrivateKey::new(&mut rand::thread_rng(), 2048).unwrap();
        let pubkey = RsaPublicKey::from(&_priv_key);
        Operator::new_with_pubkey(pubkey, op_id, address)
    }

    // fetch operator from database
    fn get_operator_from_db(db: NetworkDatabase, id: OperatorId) -> Option<Operator> {
        let conn = db.connection().unwrap();
        let mut query = conn
            .prepare("SELECT operator_id, public_key, owner_address FROM operators WHERE operator_id = ?1")
            .unwrap();
        let res: Option<(u64, String, String)> = query
            .query_row(params![*id], |row| {
                Ok((
                    row.get(0).unwrap(),
                    row.get(1).unwrap(),
                    row.get(2).unwrap(),
                ))
            })
            .ok();
        res.map(|operator| operator.into())
    }

    #[test]
    // Test inserting into the database and then confirming that it is both in
    // memory and in the underlying database
    fn test_insert_retrieve_operator() {
        // Create a temporary database
        let dir = tempdir().unwrap();
        let file = dir.path().join("db.sqlite");
        let mut db = NetworkDatabase::create(&file).unwrap();

        // Insert dummy operator data into the database
        let operator = dummy_operator();
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
        let db_operator = get_operator_from_db(db, operator.id);
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
        let operator = dummy_operator();
        let _ = db.insert_operator(&operator);

        // Now, delete the operator
        assert!(db.delete_operator(operator.id).is_ok());

        // Confirm that is it removed from in memory
        assert!(db.get_operator(&operator.id).is_none());

        // Also confirm that it is removed from the database
        assert!(get_operator_from_db(db, operator.id).is_none());
    }
}
