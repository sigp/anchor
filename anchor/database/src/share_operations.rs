use rusqlite::{Transaction, params};
use ssv_types::Share;
use types::PublicKeyBytes;

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
                *share.operator_id,
                share.share_pubkey.to_string(),
                share.encrypted_private_key
            ])?;
        Ok(())
    }
}
