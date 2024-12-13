use super::{DatabaseError, NetworkDatabase, SqlStatement, SQL};
use rusqlite::{params, Transaction};
use ssv_types::{ClusterId, OperatorId, Share};
use types::PublicKey;

/// Implements all Share related functionality on the database
impl NetworkDatabase {
    pub(crate) fn insert_share(
        &self,
        tx: &Transaction<'_>,
        share: &Share,
        cluster_id: ClusterId,
        operator_id: OperatorId,
        validator_pubkey: &PublicKey,
    ) -> Result<(), DatabaseError> {
        tx.prepare_cached(SQL[&SqlStatement::InsertShare])?
            .execute(params![
                validator_pubkey.to_string(),
                *cluster_id,
                *operator_id,
                share.share_pubkey.to_string(),
                share.encrypted_private_key
            ])?;
        Ok(())
    }
}
