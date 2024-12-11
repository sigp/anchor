use super::{DatabaseError, NetworkDatabase, SqlStatement, SQL};
use base64::prelude::*;
use openssl::pkey::Public;
use openssl::rsa::Rsa;

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

        // Insert into the database, then store in memory
        let conn = self.connection()?;
        conn.prepare_cached(SQL[&SqlStatement::InsertOperator])?
            .execute(params![
                *operator.id,
                Self::encode_pubkey(&operator.rsa_pubkey),
                operator.owner.to_string()
            ])?;
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

    /// Set the id of our own operator
    pub fn set_own_id(&mut self, id: OperatorId) {
        self.state.id = Some(id);
    }

    // Helper to encode the RsaPublicKey to PEM
    fn encode_pubkey(pubkey: &Rsa<Public>) -> String {
        // this should never fail as the key has already been validated upon construction
        BASE64_STANDARD.encode(
            pubkey
                .public_key_to_pem()
                .expect("Failed to encode RsaPublicKey"),
        )
    }
}
