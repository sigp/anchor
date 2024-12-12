use super::{DatabaseError, NetworkDatabase, SqlStatement, SQL};
use base64::prelude::*;

use rusqlite::params;
use ssv_types::{Operator, OperatorId};

/// Implements all operator related functionality on the database
impl NetworkDatabase {
    /// Insert a new operator into the database
    pub fn insert_operator(&mut self, operator: &Operator) -> Result<(), DatabaseError> {
        // make sure that this operator does not already exist
        if self.state.operators.contains_key(&operator.id) {
            return Err(DatabaseError::NotFound(format!(
                "Operator with id {} not in database",
                *operator.id
            )));
        }

        // Check if this operator is us
        if self.state.id.is_none() {
            let keys_match = operator
                .rsa_pubkey
                .public_key_to_pem()
                .and_then(|key1| self.pubkey.public_key_to_pem().map(|key2| key1 == key2))
                .unwrap_or(false);
            if keys_match {
                self.state.id = Some(operator.id);
            }
        }

        // encode the key
        let encoded = BASE64_STANDARD.encode(
            operator
                .rsa_pubkey
                .public_key_to_pem()
                .expect("Failed to encode RsaPublicKey"),
        );

        // Insert into the database, then store in memory
        let conn = self.connection()?;
        conn.prepare_cached(SQL[&SqlStatement::InsertOperator])?
            .execute(params![*operator.id, encoded, operator.owner.to_string()])?;
        self.state.operators.insert(operator.id, operator.clone());
        Ok(())
    }

    /// Delete an operator
    pub fn delete_operator(&mut self, id: OperatorId) -> Result<(), DatabaseError> {
        // make sure that it exists
        if !self.state.operators.contains_key(&id) {
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
        self.state.operators.remove(&id);
        Ok(())
    }
}
