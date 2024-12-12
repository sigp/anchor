use super::{DatabaseError, NetworkDatabase, SqlStatement, SQL};
use rusqlite::params;
use ssv_types::{Cluster, ClusterId};

/// Implements all cluster related functionality on the database
impl NetworkDatabase {
    /// Inserts a new cluster into the database
    pub fn insert_cluster(&mut self, cluster: Cluster) -> Result<(), DatabaseError> {
        // Make sure this cluster does not exists
        if self.state.clusters.contains(&cluster.cluster_id) {
            return Err(DatabaseError::AlreadyPresent(format!(
                "Cluster with id {} already in database",
                *cluster.cluster_id
            )));
        }

        let mut conn = self.connection()?;
        let tx = conn.transaction()?;

        // Insert the top level cluster data and associated validator metadata
        tx.prepare_cached(SQL[&SqlStatement::InsertCluster])?
            .execute(params![*cluster.cluster_id])?;
        tx.prepare_cached(SQL[&SqlStatement::InsertValidator])?
            .execute(params![
                cluster.validator_metadata.validator_pubkey.to_string(),
                *cluster.cluster_id,
                cluster.validator_metadata.owner.to_string(),
                cluster.validator_metadata.owner.to_string(),
                *cluster.validator_metadata.validator_index,
            ])?;

        // Insert all of the members and their shares
        let mut member_in_cluster = false;
        cluster.cluster_members.iter().try_for_each(|member| {
            if let Some(id) = self.state.id {
                if id == member.operator_id {
                    member_in_cluster = true;
                }
            }
            tx.prepare_cached(SQL[&SqlStatement::InsertClusterMember])?
                .execute(params![*member.cluster_id, *member.operator_id])?;
            self.insert_share(
                &tx,
                &member.share,
                member.cluster_id,
                member.operator_id,
                &cluster.validator_metadata.validator_pubkey,
            )
        })?;

        // Commit all operations to the db
        tx.commit()?;

        // If we are a member in this cluster, store relevant information
        if member_in_cluster {
            let cluster_id = cluster.cluster_id;
            // Store the cluster_id since we are a part of this cluster
            self.state.clusters.insert(cluster_id);
            cluster.cluster_members.iter().for_each(|member| {
                // Store all of the operators that are a member of this cluster
                self.state
                    .cluster_members
                    .entry(cluster_id)
                    .or_default()
                    .insert(member.operator_id);
                // Store our share of the key
                if member.operator_id == self.state.id.expect("Guaranteed to be populated") {
                    self.state.shares.insert(cluster_id, member.share.clone());
                }
            });
            // Store the metadata of the validator for the cluster
            self.state
                .validator_metadata
                .insert(cluster_id, cluster.validator_metadata);
        }
        Ok(())
    }

    /// Mark the cluster as liquidated or active
    pub fn update_status(&mut self, id: ClusterId, status: bool) -> Result<(), DatabaseError> {
        if !self.state.clusters.contains(&id) {
            return Err(DatabaseError::NotFound(format!(
                "Cluster with id {} not in database",
                *id
            )));
        }

        let conn = self.connection()?;
        conn.prepare_cached(SQL[&SqlStatement::UpdateClusterStatus])?
            .execute(params![status, *id])?;
        Ok(())
    }

    /// Delete a cluster from the database. This will cascade and delete all corresponding cluster
    /// members, shares, and validator metadata
    /// This corresponds to a validator being removed or exiting
    pub fn delete_cluster(&mut self, id: ClusterId) -> Result<(), DatabaseError> {
        // Make sure this cluster exists
        if !self.state.clusters.contains(&id) {
            return Err(DatabaseError::NotFound(format!(
                "Cluster with id {} not in database",
                *id
            )));
        }

        let conn = self.connection()?;
        conn.prepare_cached(SQL[&SqlStatement::DeleteCluster])?
            .execute(params![*id])?;

        // If we are a member of this cluster, remove all relevant information
        if self.state.clusters.contains(&id) {
            self.state.clusters.remove(&id);
            self.state.shares.remove(&id);
            self.state.validator_metadata.remove(&id);
            self.state.cluster_members.remove(&id);
        }
        Ok(())
    }
}
