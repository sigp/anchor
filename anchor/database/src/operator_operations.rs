use super::{DatabaseError, NetworkDatabase, SqlStatement, SQL};
use base64::prelude::*;

use rusqlite::params;
use ssv_types::{Operator, OperatorId};

/// Implements all operator related functionality on the database
impl NetworkDatabase {
    /// Insert a new operator into the database
    pub fn insert_operator(&self, operator: &Operator) -> Result<(), DatabaseError> {
        // make sure that this operator does not already exist
        if self.operator_exists(&operator.id) {
            return Err(DatabaseError::NotFound(format!(
                "Operator with id {} already in database",
                *operator.id
            )));
        }

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

        // Check to see if this operator is us and insert it into db
        //self.state.operators.insert(operator.id, operator.clone());
        self.modify_state(|state| {
            if state.id.is_none() {
                let keys_match = operator
                    .rsa_pubkey
                    .public_key_to_pem()
                    .and_then(|key1| self.pubkey.public_key_to_pem().map(|key2| key1 == key2))
                    .unwrap_or(false);
                if keys_match {
                    state.id = Some(operator.id);
                }
            }

            state.operators.insert(operator.id, operator.clone());
        });
        Ok(())
    }

    /// Delete an operator
    pub fn delete_operator(&self, id: OperatorId) -> Result<(), DatabaseError> {
        // make sure that this operator exists
        if !self.operator_exists(&id) {
            return Err(DatabaseError::NotFound(format!(
                "Operator with id {} already in database",
                *id
            )));
        }

        // Remove from db and in memory. This should cascade to delete this operator from all of the
        // clusters that it is in and all of the shares that it owns
        let conn = self.connection()?;
        conn.prepare_cached(SQL[&SqlStatement::DeleteOperator])?
            .execute(params![*id])?;

        // Remove the operator
        self.modify_state(|state| state.operators.remove(&id));
        Ok(())
    }
}
