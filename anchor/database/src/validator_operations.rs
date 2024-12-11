use super::{DatabaseError, NetworkDatabase, SqlStatement, SQL};
use rusqlite::params;
use ssv_types::{ClusterId, ValidatorIndex};
use types::{Address, PublicKey};

/// Implements all validator related db functionality
impl NetworkDatabase {
    /// Update the fee recipient address for a validator
    pub fn update_fee_recipient(
        &mut self,
        cluster_id: ClusterId,
        validator_pubkey: PublicKey,
        fee_recipient: Address,
    ) -> Result<(), DatabaseError> {
        // Make sure we are part of the cluster for this Validator
        if !self.state.clusters.contains(&cluster_id) {
            return Err(DatabaseError::NotFound(format!(
                "Validator for Cluster {} not in database",
                *cluster_id
            )));
        }

        let conn = self.connection()?;
        conn.prepare_cached(SQL[&SqlStatement::UpdateFeeRecipient])?
            .execute(params![
                fee_recipient.to_string(),
                validator_pubkey.to_string()
            ])?;
        let metadata = self
            .state
            .validator_metadata
            .get_mut(&cluster_id)
            .expect("Cluster should exist");
        metadata.fee_recipient = fee_recipient;
        Ok(())
    }

    /// Set the index of the validator
    pub fn set_validator_index(
        &mut self,
        cluster_id: ClusterId,
        validator_pubkey: PublicKey,
        index: ValidatorIndex,
    ) -> Result<(), DatabaseError> {
        // Make sure we are part of the cluster for this validaor
        if !self.state.clusters.contains(&cluster_id) {
            return Err(DatabaseError::NotFound(format!(
                "Validator for Cluster {} not in database",
                *cluster_id
            )));
        }

        let conn = self.connection()?;
        conn.prepare_cached(SQL[&SqlStatement::SetValidatorIndex])?
            .execute(params![*index, validator_pubkey.to_string()])?;
        let metadata = self
            .state
            .validator_metadata
            .get_mut(&cluster_id)
            .expect("Cluster should exist");
        metadata.validator_index = index;
        Ok(())
    }
}
