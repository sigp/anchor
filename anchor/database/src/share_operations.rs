use std::{collections::HashMap, str::FromStr};

use bls::PublicKeyBytes;
use rusqlite::{Transaction, params};
use ssv_types::{OperatorId, Share};

use super::{DatabaseError, NetworkDatabase, sql_operations};

/// Implements all Share related functionality on the database
impl NetworkDatabase {
    pub(crate) fn insert_share(
        &self,
        tx: &Transaction<'_>,
        share: &Share,
        validator_pubkey: &PublicKeyBytes,
    ) -> Result<(), DatabaseError> {
        tx.prepare_cached(sql_operations::INSERT_SHARE)?
            .execute(params![
                validator_pubkey.to_string(),
                *share.cluster_id,
                share.operator_id,
                share.share_pubkey.to_string(),
                share.encrypted_private_key
            ])?;
        Ok(())
    }

    /// Fetch all operator share public keys for a given validator.
    ///
    /// Returns a map from `OperatorId` to the operator's BLS share public key.
    /// Used during signature reconstruction to verify individual shares.
    pub fn get_share_pubkeys_for_validator(
        &self,
        validator_pubkey: &PublicKeyBytes,
    ) -> Result<HashMap<OperatorId, PublicKeyBytes>, DatabaseError> {
        let conn = self.connection()?;
        let mut stmt = conn.prepare(sql_operations::GET_SHARE_PUBKEYS_FOR_VALIDATOR)?;
        let mut rows = stmt.query(params![validator_pubkey.to_string()])?;
        Self::collect_share_pubkeys(&mut rows)
    }

    /// Fetch all operator share public keys for a given validator index.
    pub fn get_share_pubkeys_for_validator_index(
        &self,
        validator_index: ssv_types::ValidatorIndex,
    ) -> Result<HashMap<OperatorId, PublicKeyBytes>, DatabaseError> {
        let conn = self.connection()?;
        let mut stmt = conn.prepare(sql_operations::GET_SHARE_PUBKEYS_FOR_VALIDATOR_INDEX)?;
        let mut rows = stmt.query(params![validator_index])?;
        Self::collect_share_pubkeys(&mut rows)
    }

    /// Decode rows of `(operator_id, share_pubkey)` into a map.
    fn collect_share_pubkeys(
        rows: &mut rusqlite::Rows<'_>,
    ) -> Result<HashMap<OperatorId, PublicKeyBytes>, DatabaseError> {
        let mut result = HashMap::new();
        while let Some(row) = rows.next()? {
            let operator_id: u64 = row.get(0)?;
            let share_pubkey_str: String = row.get(1)?;
            let share_pubkey = PublicKeyBytes::from_str(&share_pubkey_str).map_err(|e| {
                DatabaseError::SQLError(format!(
                    "Invalid share pubkey for operator {operator_id}: {e}"
                ))
            })?;
            result.insert(OperatorId(operator_id), share_pubkey);
        }
        Ok(result)
    }
}
