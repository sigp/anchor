use base64::prelude::*;
use rusqlite::{Transaction, params};
use ssv_types::{Operator, OperatorId};
use tracing::trace;

use super::{DatabaseError, NetworkDatabase, PubkeyOrId, sql_operations};

/// Represents the status of an operator in the database
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperatorStatus {
    /// Operator exists and is active (removed = false)
    Active,
    /// Operator exists but is soft deleted (removed = true)
    SoftDeleted,
    /// Operator doesn't exist in the database at all
    NotFound,
}
/// Implements all operator related functionality on the database
impl NetworkDatabase {
    pub(crate) fn insert_operator_tx(
        &self,
        operator: &Operator,
        tx: &Transaction<'_>,
    ) -> Result<(), DatabaseError> {
        // Base64 encode the key for storage
        let pem_key = operator
            .rsa_pubkey
            .public_key_to_pem()
            .expect("Failed to encode RsaPublicKey");
        let encoded = BASE64_STANDARD.encode(&pem_key);

        // Insert into the database
        tx.prepare_cached(sql_operations::INSERT_OPERATOR)?
            .execute(params![
                operator.id,                // The id of the registered operator
                encoded,                    // RSA public key
                operator.owner.to_string()  // The owner address of the operator
            ])?;

        Ok(())
    }

    pub(crate) fn apply_insert_operator_state(
        &self,
        state: &mut crate::NetworkState,
        operator: &Operator,
    ) {
        let pem_key = operator
            .rsa_pubkey
            .public_key_to_pem()
            .expect("Failed to encode RsaPublicKey");

        if state.single_state.id.is_none() {
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

        state
            .single_state
            .operators
            .insert(operator.id, operator.to_owned());
    }

    pub fn commit_operator_added(
        &self,
        operator: &Operator,
        max_operator_id_seen: u64,
        cursor: crate::ProcessedEventCursor,
    ) -> Result<(), DatabaseError> {
        let operator = operator.clone();
        self.commit_db_update(
            super::ProgressUpdate::Event(cursor),
            true,
            |tx| {
                tx.prepare_cached(sql_operations::SET_MAX_OPERATOR_ID_SEEN)?
                    .execute(params![max_operator_id_seen])?;
                self.insert_operator_tx(&operator, tx)
            },
            |state| {
                state.single_state.max_operator_id_seen = Some(max_operator_id_seen);
                self.apply_insert_operator_state(state, &operator);
            },
        )
    }

    /// Insert a new Operator into the database
    pub fn insert_operator(
        &self,
        operator: &Operator,
        tx: &Transaction<'_>,
    ) -> Result<(), DatabaseError> {
        // Make sure that this operator does not already exist
        if self.state().operator_exists(&operator.id) {
            return Err(DatabaseError::NotFound(format!(
                "Operator with id {} already in database",
                *operator.id
            )));
        }

        self.insert_operator_tx(operator, tx)?;
        self.state
            .send_modify(|state| self.apply_insert_operator_state(state, operator));
        Ok(())
    }

    pub(crate) fn delete_operator_tx(
        &self,
        id: OperatorId,
        tx: &Transaction<'_>,
    ) -> Result<(), DatabaseError> {
        if let Err(err) = tx
            .prepare_cached(sql_operations::DELETE_OPERATOR)?
            .execute(params![id])
        {
            trace!(
                ?err,
                ?id,
                "Failed to delete operator, marking as removed instead"
            );

            // Deleting failed, likely because of a foreign key restraint. The operator is still
            // member of a committee.
            // Mark the operator as removed. This will allow cluster membership to remain recorded.
            // The operator will be removed by a trigger if no cluster membership remains.
            tx.prepare_cached(sql_operations::MARK_OPERATOR_REMOVED)?
                .execute(params![id])?;
        }

        Ok(())
    }

    pub(crate) fn apply_delete_operator_state(
        &self,
        state: &mut crate::NetworkState,
        id: OperatorId,
    ) {
        state.single_state.operators.remove(&id);
    }

    pub fn commit_operator_removed(
        &self,
        id: OperatorId,
        cursor: crate::ProcessedEventCursor,
    ) -> Result<(), DatabaseError> {
        self.commit_db_update(
            super::ProgressUpdate::Event(cursor),
            true,
            |tx| self.delete_operator_tx(id, tx),
            |state| self.apply_delete_operator_state(state, id),
        )
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

        self.delete_operator_tx(id, tx)?;
        self.state
            .send_modify(|state| self.apply_delete_operator_state(state, id));
        Ok(())
    }

    /// Get the status of an operator in the database
    pub fn get_operator_status(
        &self,
        id: OperatorId,
        tx: &Transaction<'_>,
    ) -> Result<OperatorStatus, DatabaseError> {
        match tx.query_row(sql_operations::GET_OPERATOR_STATUS, params![id], |row| {
            row.get::<_, bool>(0)
        }) {
            Ok(removed) => Ok(if removed {
                OperatorStatus::SoftDeleted
            } else {
                OperatorStatus::Active
            }),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(OperatorStatus::NotFound),
            Err(e) => Err(DatabaseError::from(e)),
        }
    }

    /// Check if an operator is soft-deleted (marked as removed but still exists in database)
    pub fn is_operator_soft_deleted(
        &self,
        id: OperatorId,
        tx: &Transaction<'_>,
    ) -> Result<bool, DatabaseError> {
        Ok(matches!(
            self.get_operator_status(id, tx)?,
            OperatorStatus::SoftDeleted
        ))
    }

    /// Check if an operator exists in the database (either active or soft deleted)
    pub fn does_operator_exist(
        &self,
        id: OperatorId,
        tx: &Transaction<'_>,
    ) -> Result<bool, DatabaseError> {
        Ok(!matches!(
            self.get_operator_status(id, tx)?,
            OperatorStatus::NotFound
        ))
    }
}
