use std::collections::HashMap;

use bls::PublicKeyBytes;
use rusqlite::{Connection, Transaction, params};
use ssv_types::ValidatorIndex;
use tracing::debug;
use types::{Address, Graffiti};

use crate::{
    DatabaseError, NetworkDatabase, NonUniqueIndex, multi_index::UniqueIndex,
    parse_optional_text_column, sql_operations,
};

/// Implements all validator specific database functionality
impl NetworkDatabase {
    /// Insert or update one owner-level fee-recipient override inside an existing transaction.
    pub(crate) fn update_fee_recipient_tx(
        &self,
        owner: Address,
        fee_recipient: Address,
        tx: &Transaction<'_>,
    ) -> Result<(), DatabaseError> {
        tx.prepare_cached(sql_operations::INSERT_OR_UPDATE_OWNER_FEE_RECIPIENT)?
            .execute(params![
                owner.to_string(),         // Owner of the cluster
                fee_recipient.to_string()  // New fee recipient address for entire cluster
            ])?;

        Ok(())
    }

    /// Mirror a committed fee-recipient override into `NetworkState`.
    ///
    /// This updates both the owner-level override map and all already-materialized clusters owned
    /// by that address.
    pub(crate) fn apply_update_fee_recipient_state(
        &self,
        state: &mut crate::NetworkState,
        owner: Address,
        fee_recipient: Address,
    ) {
        state
            .single_state
            .fee_recipients
            .insert(owner, fee_recipient);
        state.multi_state.clusters.modify_all_by(&owner, |cluster| {
            cluster.fee_recipient = fee_recipient;
        });
    }

    /// Commit the durable effects of one `FeeRecipientAddressUpdated` event.
    pub fn commit_fee_recipient_updated(
        &self,
        owner: Address,
        fee_recipient: Address,
        cursor: crate::ProcessedEventCursor,
    ) -> Result<(), DatabaseError> {
        self.commit_db_update(
            super::ProgressUpdate::Event(cursor),
            true,
            |tx| self.update_fee_recipient_tx(owner, fee_recipient, tx),
            |state| self.apply_update_fee_recipient_state(state, owner, fee_recipient),
        )
    }

    /// Read the current fee recipient for an owner from the supplied committed connection view.
    fn fee_recipient_for_owner_from_conn(
        &self,
        owner: &Address,
        conn: &Connection,
    ) -> Result<Option<Address>, DatabaseError> {
        let mut stmt = conn.prepare_cached(sql_operations::GET_OWNER_FEE_RECIPIENT)?;

        let result = stmt.query_row(params![owner.to_string()], |row| {
            parse_optional_text_column(row, 0)
        });

        match result {
            Ok(address) => Ok(address),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(DatabaseError::from(e)),
        }
    }

    /// Get the fee recipient for an owner from the latest committed database state.
    pub fn fee_recipient_for_owner(
        &self,
        owner: &Address,
    ) -> Result<Option<Address>, DatabaseError> {
        let conn = self.connection()?;
        self.fee_recipient_for_owner_from_conn(owner, &conn)
    }

    /// Update one validator's graffiti inside an existing transaction.
    pub(crate) fn update_graffiti_tx(
        &self,
        validator_pubkey: &PublicKeyBytes,
        graffiti: Graffiti,
        tx: &Transaction<'_>,
    ) -> Result<(), DatabaseError> {
        // Update the database
        tx.prepare_cached(sql_operations::SET_GRAFFITI)?
            .execute(params![
                graffiti.0.as_slice(),        // New graffiti
                validator_pubkey.to_string()  // The public key of the validator
            ])?;

        Ok(())
    }

    /// Mirror a committed graffiti update into `NetworkState`.
    pub(crate) fn apply_update_graffiti_state(
        &self,
        state: &mut crate::NetworkState,
        validator_pubkey: &PublicKeyBytes,
        graffiti: Graffiti,
    ) {
        if let Some(validator) = state
            .multi_state
            .validator_metadata
            .get_mut_by(validator_pubkey)
        {
            // Update in memory
            validator.graffiti = graffiti;
        }
    }

    /// Commit a graffiti update against both SQLite and the in-memory read model.
    pub fn update_graffiti(
        &self,
        validator_pubkey: &PublicKeyBytes,
        graffiti: Graffiti,
    ) -> Result<(), DatabaseError> {
        let validator_pubkey = *validator_pubkey;
        self.commit_db_update(
            super::ProgressUpdate::None,
            true,
            |tx| self.update_graffiti_tx(&validator_pubkey, graffiti, tx),
            |state| self.apply_update_graffiti_state(state, &validator_pubkey, graffiti),
        )
    }

    /// Commit a batch of validator index updates and then mirror them into `NetworkState`.
    pub fn set_validator_indices(
        &self,
        map: HashMap<PublicKeyBytes, ValidatorIndex>,
    ) -> Result<(), DatabaseError> {
        self.commit_db_update(
            super::ProgressUpdate::None,
            true,
            |tx| self.set_validator_indices_tx(&map, tx),
            |state| self.apply_set_validator_indices_state(state, &map),
        )
    }

    /// Persist a batch of validator index updates inside an existing transaction.
    pub(crate) fn set_validator_indices_tx(
        &self,
        map: &HashMap<PublicKeyBytes, ValidatorIndex>,
        tx: &Transaction<'_>,
    ) -> Result<(), DatabaseError> {
        for (public_key, index) in map {
            tx.prepare_cached(sql_operations::SET_INDEX)?
                .execute(params![
                    index,                  // New index
                    public_key.to_string()  // The public key of the validator
                ])?;
        }

        Ok(())
    }

    /// Mirror a committed batch of validator index updates into `NetworkState`.
    pub(crate) fn apply_set_validator_indices_state(
        &self,
        state: &mut crate::NetworkState,
        map: &HashMap<PublicKeyBytes, ValidatorIndex>,
    ) {
        for (public_key, index) in map {
            if let Some(validator) = state.multi_state.validator_metadata.get_mut_by(&public_key) {
                // Update in memory
                validator.index = Some(*index);
            } else {
                // TODO: Distinguish "DB updated 0 rows because the validator was removed while
                // index sync was in flight" from real DB/cache divergence.
                // `set_validator_indices_tx` should return only the pubkeys that
                // actually updated rows so we can avoid logging expected races and
                // warn more strongly on true inconsistencies.
                debug!(?public_key, "Tried to update index of unknown validator");
            }
        }
    }
}
