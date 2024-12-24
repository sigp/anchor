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
        // Make sure the cluster exists by getting the in memory entry
        if let Some(mut cluster) = self.clusters().get_by(&owner) {
            let conn = self.connection()?;
            conn.prepare_cached(SQL[&SqlStatement::UpdateFeeRecipient])?
                .execute(params![
                    fee_recipient.to_string(), // New fee recipient address for entire cluster
                    owner.to_string()          // Owner of the cluster
                ])?;

            // Update recipient address on the entry and update it in the map
            cluster.fee_recipient = fee_recipient;
            self.clusters()
                .update(&cluster.cluster_id, cluster.to_owned());
        }
        Ok(())
    }

    /// Update the Graffiti for a Validator
    pub fn update_graffiti(
        &self,
        validator_pubkey: &PublicKey,
        graffiti: Graffiti,
    ) -> Result<(), DatabaseError> {
        // Make sure this validator exists by getting the in memory entry
        if let Some(mut validator) = self.metadata().get_by(validator_pubkey) {
            // Update the database
            let conn = self.connection()?;
            conn.prepare_cached(SQL[&SqlStatement::SetGraffiti])?
                .execute(params![
                    graffiti.0.as_slice(),        // New graffiti
                    validator_pubkey.to_string()  // The public key of the validator
                ])?;

            // Update the Graffifi field on the entry and update it in the map
            validator.graffiti = graffiti;
            self.metadata().update(validator_pubkey, validator);
        }
        Ok(())
    }
}
