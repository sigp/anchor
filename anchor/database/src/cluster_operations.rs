use rusqlite::{Transaction, params};
use ssv_types::{Cluster, ClusterId, OperatorId, Share, ValidatorMetadata};
use types::{Address, PublicKeyBytes};

use super::{
    ClusterIndexed, DatabaseError, MetadataIndexed, NetworkDatabase, ShareIndexed, sql_operations,
};

/// Implements all cluster related functionality on the database
impl NetworkDatabase {
    /// Inserts a new validator into the database. A new cluster will be created if this is the
    /// first validator for the cluster
    pub fn insert_validator(
        &self,
        cluster: Cluster,
        validator: &ValidatorMetadata,
        shares: Vec<Share>,
        tx: &Transaction<'_>,
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
                validator.index.as_deref(),       // validator index
                validator.graffiti.0.as_slice(),  // graffiti
            ])?;

        // Insert a fee recipient address if one does not already exist
        tx.execute(
            "INSERT OR IGNORE INTO owners (owner, fee_recipient) VALUES (?, ?)",
            params![cluster.owner.to_string(), cluster.owner.to_string()],
        )?;

        // Record shares if one belongs to the current operator
        let mut our_share = None;
        let own_id = self.state.borrow().single_state.id;

        shares.iter().try_for_each(|share| {
            // Check if any of these shares belong to us, meaning we are a member in the cluster
            if own_id == Some(OperatorId(*share.operator_id)) {
                our_share = Some(share);
            }

            // Insert the cluster member and the share
            tx.prepare_cached(sql_operations::INSERT_CLUSTER_MEMBER)?
                .execute(params![*share.cluster_id, *share.operator_id])?;
            self.insert_share(tx, share, &validator.public_key)
        })?;

        self.modify_state(|state| {
            // If we are a member in this cluster, store membership and our share
            if let Some(share) = our_share {
                // Record that we are a member of this cluster
                state.single_state.clusters.insert(cluster.cluster_id);

                state.multi_state.shares.insert(ShareIndexed {
                    validator_pubkey: validator.public_key,
                    cluster_id: cluster.cluster_id,
                    owner: cluster.owner,
                    committee_id: cluster.committee_id(),
                    share: share.to_owned(),
                });
            }

            // Save all cluster related information
            // Check if we already have this cluster
            let existing = state
                .multi_state
                .clusters
                .get_by_cluster_id(&cluster.cluster_id)
                .is_some();

            // Only insert if it doesn't exist yet
            if !existing {
                state.multi_state.clusters.insert(ClusterIndexed {
                    cluster_id: cluster.cluster_id,
                    owner: cluster.owner,
                    committee_id: cluster.committee_id(),
                    cluster: cluster.to_owned(),
                });
            }

            state
                .multi_state
                .validator_metadata
                .insert(MetadataIndexed {
                    validator_pubkey: validator.public_key,
                    cluster_id: cluster.cluster_id,
                    owner: cluster.owner,
                    committee_id: cluster.committee_id(),
                    metadata: validator.to_owned(),
                });
        });

        Ok(())
    }

    /// Mark the cluster as liquidated or active
    pub fn update_status(
        &self,
        cluster_id: ClusterId,
        status: bool,
        tx: &Transaction<'_>,
    ) -> Result<(), DatabaseError> {
        tx.prepare_cached(sql_operations::UPDATE_CLUSTER_STATUS)?
            .execute(params![
                status,      // status of the cluster (liquidated = false, active = true)
                *cluster_id  // Id of the cluster
            ])?;

        // Update in memory status of cluster
        self.modify_state(|state| {
            state
                .multi_state
                .clusters
                .modify_by_cluster_id(&cluster_id, |cluster_idx| {
                    cluster_idx.cluster.liquidated = status;
                });
        });

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
        // Remove from database
        tx.prepare_cached(sql_operations::DELETE_VALIDATOR)?
            .execute(params![validator_pubkey.to_string()])?;

        self.modify_state(|state| {
            // Remove from in memory
            state
                .multi_state
                .shares
                .remove_by_validator_pubkey(validator_pubkey);

            let metadata_idx = state
                .multi_state
                .validator_metadata
                .remove_by_validator_pubkey(validator_pubkey)
                .expect("Data should have existed");

            // If there is no longer and validators for this cluster, remove it from both the
            // cluster multi index map and the cluster membership set
            if state
                .multi_state
                .validator_metadata
                .get_by_cluster_id(&metadata_idx.metadata.cluster_id)
                .is_empty()
            {
                state
                    .multi_state
                    .clusters
                    .remove_by_cluster_id(&metadata_idx.metadata.cluster_id);
                state
                    .single_state
                    .clusters
                    .remove(&metadata_idx.metadata.cluster_id);
            }
        });

        Ok(())
    }

    /// Bump the nonce of the owner
    pub fn bump_and_get_nonce(
        &self,
        owner: &Address,
        tx: &Transaction<'_>,
    ) -> Result<u16, DatabaseError> {
        // bump the nonce in the db
        tx.prepare_cached(sql_operations::BUMP_NONCE)?
            .execute(params![owner.to_string()])?;

        let mut nonce = 0;
        self.modify_state(|state| {
            // bump the nonce in memory
            if !state.single_state.nonces.contains_key(owner) {
                // if it does not yet exist in memory, then create an entry and set it to zero
                state.single_state.nonces.insert(*owner, 0);
            } else {
                // otherwise, just increment the entry
                let entry = state
                    .single_state
                    .nonces
                    .get_mut(owner)
                    .expect("This must exist");
                *entry += 1;
                nonce = *entry;
            }
        });
        Ok(nonce)
    }
}
