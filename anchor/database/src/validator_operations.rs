use std::collections::HashMap;

use bls::PublicKeyBytes;
use rusqlite::{Transaction, params};
use ssv_types::ValidatorIndex;
use tracing::debug;
use types::{Address, Graffiti};

use crate::{
    DatabaseError, NetworkDatabase, NonUniqueIndex, multi_index::UniqueIndex,
    parse_optional_text_column, sql_operations,
};

/// Implements all validator specific database functionality
impl NetworkDatabase {
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

    /// Update the fee recipient address for all validators in a cluster
    pub fn update_fee_recipient(
        &self,
        owner: Address,
        fee_recipient: Address,
        tx: &Transaction<'_>,
    ) -> Result<(), DatabaseError> {
        self.update_fee_recipient_tx(owner, fee_recipient, tx)?;

        self.modify_state(|state| {
            self.apply_update_fee_recipient_state(state, owner, fee_recipient);
        });
        Ok(())
    }

    /// Get the fee recipient for an owner
    /// Returns Some(address) if found, None otherwise
    pub fn fee_recipient_for_owner(
        &self,
        owner: &Address,
        tx: &Transaction<'_>,
    ) -> Result<Option<Address>, DatabaseError> {
        let mut stmt = tx.prepare_cached(sql_operations::GET_OWNER_FEE_RECIPIENT)?;

        let result = stmt.query_row(params![owner.to_string()], |row| {
            parse_optional_text_column(row, 0)
        });

        match result {
            Ok(address) => Ok(address),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(DatabaseError::from(e)),
        }
    }

    /// Update the Graffiti for a Validator
    pub fn update_graffiti(
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

        self.modify_state(|state| {
            if let Some(validator) = state
                .multi_state
                .validator_metadata
                .get_mut_by(validator_pubkey)
            {
                // Update in memory
                validator.graffiti = graffiti;
            }
        });
        Ok(())
    }

    pub fn set_validator_indices(
        &self,
        map: HashMap<PublicKeyBytes, ValidatorIndex>,
    ) -> Result<(), DatabaseError> {
        let tx_map = map.clone();
        self.commit_db_update(
            super::ProgressUpdate::None,
            true,
            |tx| self.set_validator_indices_tx(&tx_map, tx),
            |state| self.apply_set_validator_indices_state(state, map),
        )
    }

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

    pub(crate) fn apply_set_validator_indices_state(
        &self,
        state: &mut crate::NetworkState,
        map: HashMap<PublicKeyBytes, ValidatorIndex>,
    ) {
        for (public_key, index) in map {
            if let Some(validator) = state.multi_state.validator_metadata.get_mut_by(&public_key) {
                // Update in memory
                validator.index = Some(index);
            } else {
                debug!(?public_key, "Tried to update index of unknown validator");
            }
        }
    }
}
