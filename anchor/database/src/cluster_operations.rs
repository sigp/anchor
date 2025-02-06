use super::{DatabaseError, NetworkDatabase, NonUniqueIndex, SqlStatement, UniqueIndex, SQL};
use rusqlite::params;
use ssv_types::{Cluster, ClusterId, OperatorId, Share, ValidatorMetadata};
use types::{Address, PublicKeyBytes};

/// Implements all cluster related functionality on the database
impl NetworkDatabase {
    /// Inserts a new validator into the database. A new cluster will be created if this is the
    /// first validator for the cluster
    pub fn insert_validator(
        &self,
        cluster: Cluster,
        validator: ValidatorMetadata,
        shares: Vec<Share>,
    ) -> Result<(), DatabaseError> {
        let mut conn = self.connection()?;
        let tx = conn.transaction()?;

        // Insert the top level cluster data if it does not exist, and the associated validator metadata
        tx.prepare_cached(SQL[&SqlStatement::InsertCluster])?
            .execute(params![
                *cluster.cluster_id,               // cluster id
                cluster.owner.to_string(),         // owner
                cluster.fee_recipient.to_string(), // fee recipient
            ])?;
        tx.prepare_cached(SQL[&SqlStatement::InsertValidator])?
            .execute(params![
                validator.public_key.to_string(), // validator public key
                *cluster.cluster_id,              // cluster id
                *validator.index,                 // validator index
                validator.graffiti.0.as_slice(),  // graffiti
            ])?;

        // Record shares if one belongs to the current operator
        let mut our_share = None;
        let own_id = self.state.borrow().single_state.id;

        shares.iter().try_for_each(|share| {
            // Check if any of these shares belong to us, meaning we are a member in the cluster
            if own_id == Some(OperatorId(*share.operator_id)) {
                our_share = Some(share);
            }

            // Insert the cluster member and the share
            tx.prepare_cached(SQL[&SqlStatement::InsertClusterMember])?
                .execute(params![*share.cluster_id, *share.operator_id])?;
            self.insert_share(&tx, share, &validator.public_key)
        })?;

        // Commit all operations to the db
        tx.commit()?;

        self.modify_state(|state| {
            // If we are a member in this cluster, store membership and our share
            if let Some(share) = our_share {
                // Record that we are a member of this cluster
                state.single_state.clusters.insert(cluster.cluster_id);

                // Save the keyshare
                state.multi_state.shares.insert(
                    &validator.public_key, // The validator this keyshare belongs to
                    &cluster.cluster_id,   // The id of the cluster
                    &cluster.owner,        // The owner of the cluster
                    share.to_owned(),      // The keyshare itself
                );
            }

            // Save all cluster related information
            state.multi_state.clusters.insert(
                &cluster.cluster_id,   // The id of the cluster
                &validator.public_key, // The public key of validator added to the cluster
                &cluster.owner,        // Owner of the cluster
                cluster.to_owned(),    // The Cluster and all containing information
            );

            // Save the metadata for the validators
            state.multi_state.validator_metadata.insert(
                &validator.public_key, // The public key of the validator
                &cluster.cluster_id,   // The id of the cluster the validator belongs to
                &cluster.owner,        // The owner of the cluster
                validator.to_owned(),  // The metadata of the validator
            );
        });

        Ok(())
    }

    /// Mark the cluster as liquidated or active
    pub fn update_status(&self, cluster_id: ClusterId, status: bool) -> Result<(), DatabaseError> {
        let conn = self.connection()?;
        conn.prepare_cached(SQL[&SqlStatement::UpdateClusterStatus])?
            .execute(params![
                status,      // status of the cluster (liquidated = false, active = true)
                *cluster_id  // Id of the cluster
            ])?;

        // Update in memory status of cluster
        self.modify_state(|state| {
            if let Some(mut cluster) = state.multi_state.clusters.get_by(&cluster_id) {
                cluster.liquidated = status;
                state.multi_state.clusters.update(&cluster_id, cluster);
            }
        });

        Ok(())
    }

    /// Delete a validator from a cluster. This will cascade and remove all corresponding share
    /// data for this validator. If this validator is the last one in the cluster, the cluster
    /// and all corresponding cluster members will also be removed
    pub fn delete_validator(&self, validator_pubkey: &PublicKeyBytes) -> Result<(), DatabaseError> {
        // Remove from database
        let conn = self.connection()?;
        conn.prepare_cached(SQL[&SqlStatement::DeleteValidator])?
            .execute(params![validator_pubkey.to_string()])?;

        self.modify_state(|state| {
            // Remove from in memory
            state.multi_state.shares.remove(validator_pubkey);
            let metadata = state
                .multi_state
                .validator_metadata
                .remove(validator_pubkey)
                .expect("Data should have existed");

            // If there is no longer and validators for this cluster, remove it from both the cluster
            // multi index map and the cluster membership set
            if state
                .multi_state
                .validator_metadata
                .get_all_by(&metadata.cluster_id)
                .is_none()
            {
                state.multi_state.clusters.remove(&metadata.cluster_id);
                state.single_state.clusters.remove(&metadata.cluster_id);
            }
        });

        Ok(())
    }

    /// Bump the nonce of the owner
    pub fn bump_and_get_nonce(&self, owner: &Address) -> Result<u16, DatabaseError> {
        // bump the nonce in the db
        let conn = self.connection()?;
        conn.prepare_cached(SQL[&SqlStatement::BumpNonce])?
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
