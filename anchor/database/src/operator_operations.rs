use super::{DatabaseError, NetworkDatabase, SqlStatement, SQL};
use base64::prelude::*;
use rusqlite::params;
use ssv_types::{Operator, OperatorId};
use std::sync::atomic::Ordering;

/// Implements all operator related functionality on the database
impl NetworkDatabase {
    /// Insert a new Operator into the database
    pub fn insert_operator(&self, operator: &Operator) -> Result<(), DatabaseError> {
        // 1ake sure that this operator does not already exist
        if self.operator_exists(&operator.id) {
            return Err(DatabaseError::NotFound(format!(
                "Operator with id {} already in database",
                *operator.id
            )));
        }

        // Base64 encode the key for storage
        let pem_key = operator
            .rsa_pubkey
            .public_key_to_pem()
            .expect("Failed to encode RsaPublicKey");
        let encoded = BASE64_STANDARD.encode(pem_key.clone());

        // Insert into the database
        let conn = self.connection()?;
        conn.prepare_cached(SQL[&SqlStatement::InsertOperator])?
            .execute(params![
                *operator.id,               // The id of the registered operator
                encoded,                    // RSA public key
                operator.owner.to_string()  // The owner address of the operator
            ])?;

        // Check to see if this operator is the current operator
        let own_id = self.state.single_state.id.load(Ordering::Relaxed);
        if own_id == u64::MAX {
            // If the keys match, this is the current operator so we want to save the id
            let keys_match = pem_key == self.pubkey.public_key_to_pem().unwrap_or_default();
            if keys_match {
                self.state
                    .single_state
                    .id
                    .store(*operator.id, Ordering::Relaxed);
            }
        }
        // Store the operator in memory
        self.state
            .single_state
            .operators
            .insert(operator.id, operator.to_owned());
        Ok(())
    }

    /// Delete an operator
    pub fn delete_operator(&self, id: OperatorId) -> Result<(), DatabaseError> {
        // Make sure that this operator exists
        if !self.operator_exists(&id) {
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

        // Remove the operator
        self.state.single_state.operators.remove(&id);
        Ok(())
    }
}
