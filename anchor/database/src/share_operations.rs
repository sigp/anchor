use super::NetworkDatabase;
use rusqlite::{params, Transaction};
use ssv_types::{ClusterId, OperatorId, Share};
use types::PublicKey;

/// Implements all Share related functionality on the database
impl NetworkDatabase {
    pub(crate) fn insert_share(
        &mut self,
        tx: &Transaction<'_>,
        share: &Share,
        cluster_id: &ClusterId,
        operator_id: &OperatorId,
        validator_pubkey: &PublicKey,
    ) -> Result<(), String> {
        let cluster_id_i64: i64 = cluster_id.0 as i64;
        let operator_id_i64: i64 = operator_id.0 as i64;
        tx.execute(
            "INSERT INTO shares (
                    validator_pubkey,
                    cluster_id,
                    operator_id,
                    share_pubkey
                ) VALUES (?1, ?2, ?3, ?4)",
            params![
                validator_pubkey.to_string(),
                cluster_id_i64,
                operator_id_i64,
                share.share_pubkey.to_string(),
            ],
        )
        .map_err(|e| format!("Failed to insert share: {:?}", e))?;

        Ok(())
    }

    /// Get the share owned by the operator
    pub fn get_share(&self, share_pubkey: &PublicKey) -> Option<Share> {
        self.shares.get(share_pubkey).cloned()
    }

    /// Check to see if our operator owns this share
    pub fn operator_owns(&self, share_pubkey: &PublicKey) -> bool {
        self.shares.contains_key(share_pubkey)
    }
}
