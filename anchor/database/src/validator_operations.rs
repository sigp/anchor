use super::{DatabaseError, NetworkDatabase, SqlStatement, SQL};
use rusqlite::params;
use ssv_types::{ClusterId, ValidatorIndex, ValidatorMetadata};
use types::{Address, PublicKey};

/// Implements all validator related db functionality
impl NetworkDatabase {
    /// Populates or updates the fee recipient for the validator
    pub fn update_fee_recipient(
        &mut self,
        validator_pubkey: PublicKey,
        fee_recipient: Address,
    ) -> Result<(), DatabaseError> {
        let conn = self.connection()?;
        conn.prepare_cached(SQL[&SqlStatement::UpdateFeeRecipient])?
            .execute(params![
                validator_pubkey.to_string(),
                fee_recipient.to_string()
            ])?;
        Ok(())
    }

    /// Set the index of the validator
    pub fn set_validator_index(
        &mut self,
        validator_pubkey: PublicKey,
        index: ValidatorIndex,
    ) -> Result<(), DatabaseError> {
        let conn = self.connection()?;
        conn.prepare_cached(SQL[&SqlStatement::SetValidatorIndex])?
            .execute(params![validator_pubkey.to_string(), *index])?;
        Ok(())
    }

    /// Get the metatdata for the cluster
    pub fn get_validator_metadata(&self, id: &ClusterId) -> Option<&ValidatorMetadata> {
        self.state.validator_metadata.get(id)
    }
}

#[cfg(test)]
mod validator_database_tests {}
