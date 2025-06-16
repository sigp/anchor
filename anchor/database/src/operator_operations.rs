use base64::prelude::*;
use rusqlite::{Transaction, params};
use ssv_types::{Operator, OperatorId};

use super::{DatabaseError, NetworkDatabase, PubkeyOrId, SQL, SqlStatement};

/// Implements all operator related functionality on the database
impl NetworkDatabase {
    /// Insert a new Operator into the database
    pub fn insert_operator(
        &self,
        operator: &Operator,
        tx: &Transaction<'_>,
    ) -> Result<(), DatabaseError> {
        // 1ake sure that this operator does not already exist
        if self.state().operator_exists(&operator.id) {
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
        tx.prepare_cached(SQL[&SqlStatement::InsertOperator])?
            .execute(params![
                *operator.id,               // The id of the registered operator
                encoded,                    // RSA public key
                operator.owner.to_string()  // The owner address of the operator
            ])?;

        self.state.send_modify(|state| {
            // Check to see if this operator is the current operator
            if state.single_state.id.is_none() {
                // If the keys match, this is the current operator so we want to save the id
                let keys_match = match &self.operator {
                    PubkeyOrId::Pubkey(pubkey) => {
                        pem_key == pubkey.public_key_to_pem().unwrap_or_default()
                    }
                    PubkeyOrId::Id(id) => *id == operator.id,
                };
                if keys_match {
                    state.single_state.id = Some(operator.id);
                }
            }
            // Store the operator in memory
            state
                .single_state
                .operators
                .insert(operator.id, operator.to_owned());
        });
        Ok(())
    }

    /// Delete an operator
    pub fn delete_operator(
        &self,
        id: OperatorId,
        tx: &Transaction<'_>,
    ) -> Result<(), DatabaseError> {
        // Make sure that this operator exists
        if !self.state().operator_exists(&id) {
            return Err(DatabaseError::NotFound(format!(
                "Operator with id {} not in database",
                *id
            )));
        }

        // Remove from db and in memory. This should cascade to delete this operator from all of the
        // clusters that it is in and all of the shares that it owns
        tx.prepare_cached(SQL[&SqlStatement::DeleteOperator])?
            .execute(params![*id])?;

        self.state.send_modify(|state| {
            // Remove the operator
            state.single_state.operators.remove(&id);
        });
        Ok(())
    }
}
