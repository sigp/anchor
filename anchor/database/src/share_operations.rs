use super::{DatabaseError, NetworkDatabase, SqlStatement, SQL};
use rusqlite::{params, Transaction};
use ssv_types::Share;
use types::PublicKey;

/// Implements all Share related functionality on the database
impl NetworkDatabase {
    pub(crate) fn insert_share(
        &self,
        tx: &Transaction<'_>,
        share: &Share,
        validator_pubkey: &PublicKey,
    ) -> Result<(), DatabaseError> {
        tx.prepare_cached(SQL[&SqlStatement::InsertShare])?
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
