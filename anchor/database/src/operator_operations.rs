use base64::prelude::*;
use openssl::{pkey::Public, rsa::Rsa};
use rusqlite::{OptionalExtension, Transaction, params};
use ssv_types::{Operator, OperatorId};
use tracing::trace;

use super::{DatabaseError, NetworkDatabase, PendingStateUpdates, PubkeyOrId, sql_operations};

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
    /// Insert a new operator in the active transaction and queue the matching state update.
    pub fn insert_operator_tx(
        &self,
        operator: &Operator,
        tx: &Transaction<'_>,
        state_updates: &mut PendingStateUpdates,
    ) -> Result<(), DatabaseError> {
        // Make sure that this operator does not already exist
        if self.operator_exists_tx(operator.id, tx)? {
            return Err(DatabaseError::NotFound(format!(
                "Operator with id {} already in database",
                *operator.id
            )));
        }

        // Base64 encode the key for storage
        let pem_key = operator.rsa_pubkey.public_key_to_pem()?;
        let encoded = BASE64_STANDARD.encode(&pem_key);
        let is_own_operator = match &self.operator {
            PubkeyOrId::Pubkey(pubkey) => pem_key == pubkey.public_key_to_pem()?,
            PubkeyOrId::Id(id) => *id == operator.id,
        };

        // Insert into the database
        tx.prepare_cached(sql_operations::INSERT_OPERATOR)?
            .execute(params![
                operator.id,                // The id of the registered operator
                encoded,                    // RSA public key
                operator.owner.to_string()  // The owner address of the operator
            ])?;
        state_updates.insert_operator(operator.to_owned(), is_own_operator);
        Ok(())
    }

    /// Record that an `OperatorAdded` log was intentionally skipped after consuming its
    /// monotonically-increasing operator id.
    pub fn insert_skipped_operator_add_tx(
        &self,
        id: OperatorId,
        reason: &str,
        tx: &Transaction<'_>,
    ) -> Result<(), DatabaseError> {
        tx.prepare_cached(sql_operations::INSERT_SKIPPED_OPERATOR_ADD)?
            .execute(params![id, reason])?;
        Ok(())
    }

    /// Delete a previously recorded skipped `OperatorAdded` marker.
    pub fn delete_skipped_operator_add_tx(
        &self,
        id: OperatorId,
        tx: &Transaction<'_>,
    ) -> Result<bool, DatabaseError> {
        let rows = tx
            .prepare_cached(sql_operations::DELETE_SKIPPED_OPERATOR_ADD)?
            .execute(params![id])?;
        Ok(rows > 0)
    }

    /// Check if an `OperatorAdded` log for this id was previously skipped.
    #[cfg(any(test, feature = "test-utils"))]
    pub fn was_operator_add_skipped_tx(
        &self,
        id: OperatorId,
        tx: &Transaction<'_>,
    ) -> Result<bool, DatabaseError> {
        tx.prepare_cached(sql_operations::GET_SKIPPED_OPERATOR_ADD_REASON)?
            .query_row(params![id], |row| row.get::<_, String>(0))
            .optional()
            .map(|reason| reason.is_some())
            .map_err(DatabaseError::from)
    }

    /// Resolve any operator id that already owns this canonical public key, including soft-deleted
    /// rows that still occupy the schema-level uniqueness slot.
    pub fn get_any_operator_id_by_public_key_tx(
        &self,
        public_key: &Rsa<Public>,
        tx: &Transaction<'_>,
    ) -> Result<Option<OperatorId>, DatabaseError> {
        let encoded = BASE64_STANDARD.encode(public_key.public_key_to_pem()?);
        tx.prepare_cached(sql_operations::GET_ANY_OPERATOR_ID)?
            .query_row(params![encoded], |row| row.get(0))
            .optional()
            .map_err(DatabaseError::from)
    }

    /// Delete an operator in the active transaction and queue the matching state update.
    pub fn delete_operator_tx(
        &self,
        id: OperatorId,
        tx: &Transaction<'_>,
        state_updates: &mut PendingStateUpdates,
    ) -> Result<(), DatabaseError> {
        // Make sure that this operator exists
        if !self.operator_exists_tx(id, tx)? {
            return Err(DatabaseError::NotFound(format!(
                "Operator with id {} not in database",
                *id
            )));
        }

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

        state_updates.delete_operator(id);
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

    /// Check if an operator exists in the active transaction and is not soft-deleted.
    pub fn operator_exists_tx(
        &self,
        id: OperatorId,
        tx: &Transaction<'_>,
    ) -> Result<bool, DatabaseError> {
        Ok(matches!(
            self.get_operator_status(id, tx)?,
            OperatorStatus::Active
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
