use crate::{multi_index::UniqueIndex, DatabaseError, NetworkDatabase, SqlStatement, SQL};
use rusqlite::params;
use types::{Address, Graffiti, PublicKeyBytes};

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
        conn.prepare_cached(SQL[&SqlStatement::UpdateFeeRecipient])?
            .execute(params![
                fee_recipient.to_string(), // New fee recipient address for entire cluster
                owner.to_string()          // Owner of the cluster
            ])?;

        self.modify_state(|state| {
            if let Some(mut cluster) = state.multi_state.clusters.get_by(&owner) {
                // Update in memory
                cluster.fee_recipient = fee_recipient;
                state
                    .multi_state
                    .clusters
                    .update(&cluster.cluster_id.clone(), cluster);
            }
        });
        Ok(())
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
}
