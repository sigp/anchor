use bls::PublicKeyBytes;
use rusqlite::{OptionalExtension, Transaction, params};
use ssv_types::{Cluster, ClusterId, ClusterMember, OperatorId, Share, ValidatorMetadata};
use types::Address;

use super::{DatabaseError, NetworkDatabase, PendingStateUpdates, sql_operations};

/// Implements all cluster related functionality on the database
impl NetworkDatabase {
    /// Inserts a validator in the active transaction and queues the matching state update.
    pub fn insert_validator_tx(
        &self,
        cluster: Cluster,
        validator: &ValidatorMetadata,
        shares: Vec<Share>,
        tx: &Transaction<'_>,
        state_updates: &mut PendingStateUpdates,
    ) -> Result<(), DatabaseError> {
        // Insert the top level cluster data if it does not exist, and the associated validator
        // metadata
        tx.prepare_cached(sql_operations::INSERT_CLUSTER)?
            .execute(params![
                *cluster.cluster_id,       // cluster id
                cluster.owner.to_string(), // owner
            ])?;
        tx.prepare_cached(sql_operations::INSERT_VALIDATOR)?
            .execute(params![
                validator.public_key.to_string(), // validator public key
                *cluster.cluster_id,              // cluster id
                validator.index,                  // validator index
                validator.graffiti.0.as_slice(),  // graffiti
            ])?;

        // Insert a fee recipient address if one does not already exist
        tx.execute(
            "INSERT OR IGNORE INTO owners (owner, fee_recipient) VALUES (?, ?)",
            params![cluster.owner.to_string(), cluster.owner.to_string()],
        )?;

        // Record shares if one belongs to the current operator
        let mut our_share = None;
        let own_id = self.get_own_operator_id_tx(tx)?;

        shares.iter().try_for_each(|share| {
            // Check if any of these shares belong to us, meaning we are a member in the cluster
            if own_id == Some(OperatorId(*share.operator_id)) {
                our_share = Some(share.to_owned());
            }

            // Insert the cluster member and the share
            tx.prepare_cached(sql_operations::INSERT_CLUSTER_MEMBER)?
                .execute(params![*share.cluster_id, share.operator_id])?;
            self.insert_share(tx, share, &validator.public_key)
        })?;

        state_updates.insert_validator(cluster, validator.to_owned(), our_share);

        Ok(())
    }

    /// Inserts a new validator into the database. A new cluster will be created if this is the
    /// first validator for the cluster
    pub fn insert_validator(
        &self,
        cluster: Cluster,
        validator: &ValidatorMetadata,
        shares: Vec<Share>,
        tx: &Transaction<'_>,
    ) -> Result<(), DatabaseError> {
        let mut state_updates = PendingStateUpdates::default();
        self.insert_validator_tx(cluster, validator, shares, tx, &mut state_updates)?;
        self.apply_pending_state_updates(state_updates);
        Ok(())
    }

    /// Mark the cluster as liquidated or active in the active transaction and queue the matching
    /// state update.
    pub fn update_status_tx(
        &self,
        cluster_id: ClusterId,
        status: bool,
        tx: &Transaction<'_>,
        state_updates: &mut PendingStateUpdates,
    ) -> Result<(), DatabaseError> {
        tx.prepare_cached(sql_operations::UPDATE_CLUSTER_STATUS)?
            .execute(params![
                status,      // status of the cluster (liquidated = false, active = true)
                *cluster_id  // Id of the cluster
            ])?;

        state_updates.update_cluster_status(cluster_id, status);

        Ok(())
    }

    /// Mark the cluster as liquidated or active
    pub fn update_status(
        &self,
        cluster_id: ClusterId,
        status: bool,
        tx: &Transaction<'_>,
    ) -> Result<(), DatabaseError> {
        let mut state_updates = PendingStateUpdates::default();
        self.update_status_tx(cluster_id, status, tx, &mut state_updates)?;
        self.apply_pending_state_updates(state_updates);
        Ok(())
    }

    /// Delete a validator in the active transaction and queue the matching state update.
    pub fn delete_validator_tx(
        &self,
        validator_pubkey: &PublicKeyBytes,
        tx: &Transaction<'_>,
        state_updates: &mut PendingStateUpdates,
    ) -> Result<(), DatabaseError> {
        // Remove from database
        tx.prepare_cached(sql_operations::DELETE_VALIDATOR)?
            .execute(params![validator_pubkey.to_string()])?;

        state_updates.delete_validator(*validator_pubkey);

        Ok(())
    }

    /// Delete a validator from a cluster. This will cascade and remove all corresponding share
    /// data for this validator. If this validator is the last one in the cluster, the cluster
    /// and all corresponding cluster members will also be removed
    pub fn delete_validator(
        &self,
        validator_pubkey: &PublicKeyBytes,
        tx: &Transaction<'_>,
    ) -> Result<(), DatabaseError> {
        let mut state_updates = PendingStateUpdates::default();
        self.delete_validator_tx(validator_pubkey, tx, &mut state_updates)?;
        self.apply_pending_state_updates(state_updates);
        Ok(())
    }

    /// Load a cluster by validator public key through the transaction's view of the database.
    pub fn get_cluster_by_validator_tx(
        &self,
        validator_pubkey: &PublicKeyBytes,
        tx: &Transaction<'_>,
    ) -> Result<Option<Cluster>, DatabaseError> {
        let Some(cluster_id) = tx
            .prepare_cached(sql_operations::GET_CLUSTER_BY_VALIDATOR)?
            .query_row(params![validator_pubkey.to_string()], |row| {
                Ok(ClusterId(row.get("cluster_id")?))
            })
            .optional()?
        else {
            return Ok(None);
        };

        let cluster_members = tx
            .prepare_cached(sql_operations::GET_CLUSTER_MEMBERS)?
            .query_map([cluster_id.0], |row| {
                Ok(ClusterMember {
                    cluster_id,
                    operator_id: row.get(0)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        tx.prepare_cached(sql_operations::GET_CLUSTER_BY_VALIDATOR)?
            .query_row(params![validator_pubkey.to_string()], |row| {
                Cluster::try_from((row, cluster_members))
            })
            .optional()
            .map_err(DatabaseError::from)
    }

    /// Bump the nonce of the owner in the active transaction and queue the matching state update.
    pub fn bump_and_get_nonce_tx(
        &self,
        owner: &Address,
        tx: &Transaction<'_>,
        state_updates: &mut PendingStateUpdates,
    ) -> Result<u16, DatabaseError> {
        // bump the nonce in the db
        tx.prepare_cached(sql_operations::BUMP_NONCE)?
            .execute(params![owner.to_string()])?;

        let nonce = tx
            .prepare_cached(sql_operations::GET_NONCE)?
            .query_row(params![owner.to_string()], |row| row.get(0))?;

        state_updates.set_owner_nonce(*owner, nonce);
        Ok(nonce)
    }

    /// Bump the nonce of the owner
    pub fn bump_and_get_nonce(
        &self,
        owner: &Address,
        tx: &Transaction<'_>,
    ) -> Result<u16, DatabaseError> {
        let mut state_updates = PendingStateUpdates::default();
        let nonce = self.bump_and_get_nonce_tx(owner, tx, &mut state_updates)?;
        self.apply_pending_state_updates(state_updates);
        Ok(nonce)
    }
}
