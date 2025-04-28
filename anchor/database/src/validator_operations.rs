use std::{collections::HashMap, str::FromStr};

use rusqlite::params;
use ssv_types::ValidatorIndex;
use tracing::warn;
use types::{Address, Graffiti, PublicKeyBytes};

use crate::{
    multi_index::UniqueIndex, DatabaseError, NetworkDatabase, NonUniqueIndex, SqlStatement, SQL,
};

/// Implements all validator specific database functionality
impl NetworkDatabase {
    /// Update the fee recipient address for all validators in a cluster
    pub fn update_fee_recipient(
        &self,
        owner: Address,
        fee_recipient: Address,
    ) -> Result<(), DatabaseError> {
        // Update the database
        let conn = self.connection()?;
        conn.prepare_cached(SQL[&SqlStatement::InsertOrUpdateOwnerFeeRecipient])?
            .execute(params![
                owner.to_string(),         // Owner of the cluster
                fee_recipient.to_string()  // New fee recipient address for entire cluster
            ])?;

        self.modify_state(|state| {
            if let Some(clusters) = state.multi_state.clusters.get_all_by(&owner) {
                for mut cluster in clusters {
                    // Update in memory
                    cluster.fee_recipient = fee_recipient;
                    state
                        .multi_state
                        .clusters
                        .update(&cluster.cluster_id.clone(), cluster);
                }
            }
        });
        Ok(())
    }

    /// Get the fee recipient for an owner
    /// Returns Some(address) if found, None otherwise
    pub fn fee_recipient_for_owner(
        &self,
        owner: &Address,
    ) -> Result<Option<Address>, DatabaseError> {
        let conn = self.connection()?;
        let mut stmt = conn.prepare_cached(SQL[&SqlStatement::GetOwnerFeeRecipient])?;

        let result = stmt.query_row(params![owner.to_string()], |row| {
            let address_str: String = row.get(0)?;
            let address = Address::from_str(&address_str).map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })?;
            Ok(address)
        });

        match result {
            Ok(address) => Ok(Some(address)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(DatabaseError::from(e)),
        }
    }

    /// Update the Graffiti for a Validator
    pub fn update_graffiti(
        &self,
        validator_pubkey: &PublicKeyBytes,
        graffiti: Graffiti,
    ) -> Result<(), DatabaseError> {
        // Update the database
        let conn = self.connection()?;
        conn.prepare_cached(SQL[&SqlStatement::SetGraffiti])?
            .execute(params![
                graffiti.0.as_slice(),        // New graffiti
                validator_pubkey.to_string()  // The public key of the validator
            ])?;

        self.modify_state(|state| {
            if let Some(mut validator) = state
                .multi_state
                .validator_metadata
                .get_by(validator_pubkey)
            {
                // Update in memory
                validator.graffiti = graffiti;
                state
                    .multi_state
                    .validator_metadata
                    .update(validator_pubkey, validator);
            }
        });
        Ok(())
    }

    pub fn set_validator_indices(
        &self,
        map: HashMap<PublicKeyBytes, ValidatorIndex>,
    ) -> Result<(), DatabaseError> {
        // Update the database
        let mut conn = self.connection()?;
        let transaction = conn.transaction()?;
        for (public_key, index) in map.iter() {
            transaction
                .prepare_cached(SQL[&SqlStatement::SetIndex])?
                .execute(params![
                    index.0,                // New index
                    public_key.to_string()  // The public key of the validator
                ])?;
        }
        transaction.commit()?;

        self.modify_state(|state| {
            for (public_key, index) in map {
                if let Some(mut validator) =
                    state.multi_state.validator_metadata.get_by(&public_key)
                {
                    // Update in memory
                    validator.index = Some(index);
                    state
                        .multi_state
                        .validator_metadata
                        .update(&public_key, validator);
                } else {
                    warn!(?public_key, "Tried to update index of unknown validator");
                }
            }
        });
        Ok(())
    }
}
