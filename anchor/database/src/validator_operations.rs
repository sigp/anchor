use crate::{multi_index::UniqueIndex, DatabaseError, NetworkDatabase, SqlStatement, SQL};
use rusqlite::params;
use types::{Address, Graffiti, PublicKey};

/// Implements all validator related database functionality
impl NetworkDatabase {
    /// Update the fee recipient address for all validators in a cluster
    pub fn update_fee_recipient(
        &self,
        owner: Address,
        fee_recipient: Address,
    ) -> Result<(), DatabaseError> {
        let conn = self.connection()?;
        conn.prepare_cached(SQL[&SqlStatement::UpdateFeeRecipient])?
            .execute(params![
                fee_recipient.to_string(), // new fee recipient address for entire cluster
                owner.to_string()          // owner of the cluster
            ])?;

        // If we are in the cluster, update the in memory fee recipient for the cluster
        if let Some(mut cluster) = self.state.multi_state.clusters.get_by(&owner) {
            // update recipient and insert back in to update
            cluster.fee_recipient = fee_recipient;
            self.state
                .multi_state
                .clusters
                .update(&cluster.cluster_id, cluster.to_owned());
        }
        Ok(())
    }

    /// Update the graffiti for a validator
    pub fn update_graffiti(
        &self,
        validator_pubkey: &PublicKey,
        graffiti: Graffiti,
    ) -> Result<(), DatabaseError> {
        // Update the database
        let conn = self.connection()?;
        conn.prepare_cached(SQL[&SqlStatement::SetGraffiti])?
            .execute(params![
                graffiti.0.as_slice(),        // new graffiti
                validator_pubkey.to_string()  // the public key of the validator
            ])?;

        // If we are an operator for the validator, update the in memory grafitti
        if let Some(mut validator) = self
            .state
            .multi_state
            .validator_metadata
            .get_by(validator_pubkey)
        {
            // update graffiti and insert back in to update
            validator.graffiti = graffiti;
            self.state
                .multi_state
                .validator_metadata
                .update(validator_pubkey, validator);
        }
        Ok(())
    }
}
