use super::{DatabaseError, NetworkDatabase, SqlStatement, SQL};
use rusqlite::params;
use ssv_types::ClusterId;
use types::{Address, Graffiti, PublicKey};

/// Implements all validator related db functionality
impl NetworkDatabase {
    /// Update the fee recipient address for a validator
    pub fn update_fee_recipient(
        &self,
        cluster_id: ClusterId,
        validator_pubkey: PublicKey,
        fee_recipient: Address,
    ) -> Result<(), DatabaseError> {
        // Make sure we are part of the cluster for this Validator
        let is_member = self.read_state(|state| state.clusters.contains(&cluster_id));
        if !is_member {
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

        self.modify_state(|state| {
            let metadata = state
                .validator_metadata
                .get_mut(&cluster_id)
                .expect("Cluster should exist");
            metadata.fee_recipient = fee_recipient;
        });

        Ok(())
    }

    /// Update the graffiti for a validator
    pub fn update_graffiti(
        &self,
        cluster_id: ClusterId,
        validator_pubkey: PublicKey,
        graffiti: Graffiti,
    ) -> Result<(), DatabaseError> {
        let is_member = self.read_state(|state| state.clusters.contains(&cluster_id));
        if !is_member {
            return Err(DatabaseError::NotFound(format!(
                "Validator for Cluster {} not in database",
                *cluster_id
            )));
        }

        // Update the database
        let conn = self.connection()?;
        conn.prepare_cached(SQL[&SqlStatement::SetGraffiti])?
            .execute(params![
                graffiti.0.as_slice(), // Convert [u8; 32] to &[u8]
                validator_pubkey.to_string()
            ])?;

        // Update the in-memory state
        self.modify_state(|state| {
            let metadata = state
                .validator_metadata
                .get_mut(&cluster_id)
                .expect("Cluster should exist");
            metadata.graffiti = graffiti;
        });

        Ok(())
    }
}
