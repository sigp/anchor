use super::{DatabaseError, NetworkDatabase};
use rusqlite::{params, Transaction};
use ssv_types::{ClusterId, OperatorId, Share};
use types::PublicKey;

/// Implements all Share related functionality on the database
impl NetworkDatabase {
    pub(crate) fn insert_share(
        &mut self,
        tx: &Transaction<'_>,
        share: &Share,
        cluster_id: ClusterId,
        operator_id: OperatorId,
        validator_pubkey: &PublicKey,
    ) -> Result<(), DatabaseError> {
        tx.execute(
            "INSERT INTO shares (
                    validator_pubkey,
                    cluster_id,
                    operator_id,
                    share_pubkey
                ) VALUES (?1, ?2, ?3, ?4)",
            params![
                validator_pubkey.to_string(),
                *cluster_id,
                *operator_id,
                share.share_pubkey.to_string(),
            ],
        )?;

        Ok(())
    }
}
