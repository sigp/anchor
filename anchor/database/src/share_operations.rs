use bls::PublicKeyBytes;
use rusqlite::{OptionalExtension, Transaction, params};
use ssv_types::Share;

use super::{DatabaseError, NetworkDatabase, sql_operations};

/// Implements all Share related functionality on the database
impl NetworkDatabase {
    /// Check whether the current operator has a share for `validator_pubkey` in the active
    /// transaction.
    pub fn has_own_share_tx(
        &self,
        validator_pubkey: &PublicKeyBytes,
        tx: &Transaction<'_>,
    ) -> Result<bool, DatabaseError> {
        let Some(operator_id) = self.get_own_operator_id_tx(tx)? else {
            return Ok(false);
        };

        let exists = tx
            .prepare_cached(sql_operations::HAS_OPERATOR_SHARE_FOR_VALIDATOR)?
            .query_row(params![validator_pubkey.to_string(), operator_id], |_| {
                Ok(())
            })
            .optional()?
            .is_some();
        Ok(exists)
    }

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
}
