use super::{DatabaseError, NetworkDatabase, SqlStatement, SQL};
use rusqlite::params;
use ssv_types::{Cluster, ClusterId};
use std::collections::{HashMap, HashSet};

/// Implements all cluster related functionality on the database
impl NetworkDatabase {
    /// Inserts a new cluster into the database
    pub fn insert_cluster(&mut self, cluster: Cluster) -> Result<(), DatabaseError> {
        let mut conn = self.connection()?;
        let tx = conn.transaction()?;

        // Insert the top level cluster data and associated validator metadata
        tx.prepare_cached(SQL[&SqlStatement::InsertCluster])?
            .execute(params![*cluster.cluster_id, 0])?;
        tx.prepare_cached(SQL[&SqlStatement::InsertValidator])?
            .execute(params![
                cluster.validator_metadata.validator_pubkey.to_string(),
                *cluster.cluster_id
            ])?;

        // Insert all of the members and their shares
        cluster.cluster_members.iter().try_for_each(|member| {
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

        // Since we have successfully committed, we can now store everything in memory
        self.clusters.insert(cluster.cluster_id);
        self.validator_metadata
            .insert(cluster.cluster_id, cluster.validator_metadata);

        let mut shares = HashMap::with_capacity(cluster.cluster_members.len());
        let mut members = HashSet::with_capacity(cluster.cluster_members.len());

        // Process all members in a single iteration
        for member in cluster.cluster_members {
            shares.insert(member.operator_id, member.share);
            members.insert(member.operator_id);
        }

        // Bulk insert the processed data
        self.shares.insert(cluster.cluster_id, shares);
        self.cluster_members.insert(cluster.cluster_id, members);

        Ok(())
    }

    /// Mark the cluster as liquidated or active
    pub fn update_status(&mut self, id: ClusterId, status: bool) -> Result<(), DatabaseError> {
        if !self.clusters.contains(&id) {
            return Err(DatabaseError::NotFound(format!(
                "Cluster with id {} not in database",
                *id
            )));
        }

        let conn = self.connection()?;
        conn.prepare_cached(SQL[&SqlStatement::UpdateClusterStatus])?
            .execute(params![status, *id])?;
        // todo!() change in memory status
        Ok(())
    }

    /// Update the number of fauly nodes in the cluster
    pub fn update_faulty(&mut self, id: ClusterId, num_faulty: u64) -> Result<(), DatabaseError> {
        if !self.clusters.contains(&id) {
            return Err(DatabaseError::NotFound(format!(
                "Cluster with id {} not in database",
                *id
            )));
        }

        let conn = self.connection()?;
        conn.prepare_cached(SQL[&SqlStatement::UpdateClusterFaulty])?
            .execute(params![num_faulty, *id])?;
        // todo!() change in memory status
        Ok(())
    }

    /// Delete a cluster from the database. This will cascade and delete all corresponding cluster
    /// members, shares, and validator metadata
    /// This corresponds to a validator being removed or exiting
    pub fn delete_cluster(&mut self, id: ClusterId) -> Result<(), DatabaseError> {
        // Make sure this cluster exists
        if !self.clusters.contains(&id) {
            return Err(DatabaseError::NotFound(format!(
                "Cluster with id {} not in database",
                *id
            )));
        }

        let conn = self.connection()?;
        conn.prepare_cached(SQL[&SqlStatement::DeleteCluster])?
            .execute(params![*id])?;

        // remove all in memory stores: todo!() need to figure out exactly how to structure in
        // memory
        let _ = self.clusters.remove(&id);
        Ok(())
    }

    /// Check if this cluster exists
    pub fn cluster_exists(&self, id: &ClusterId) -> bool {
        self.clusters.contains(id)
    }
}

#[cfg(test)]
mod cluster_database_tests {
    use super::*;
    use crate::test_utils::{
        db_with_cluster, dummy_cluster, dummy_operator, get_cluster_from_db,
        get_cluster_member_from_db, get_shares_from_db, get_validator_from_db,
    };
    use tempfile::tempdir;

    #[test]
    // Test inserting a cluster into the database
    fn test_insert_retrieve_cluster() {
        // Create a temporary database
        let dir = tempdir().unwrap();
        let file = dir.path().join("db.sqlite");
        let mut db = NetworkDatabase::create(&file).unwrap();

        // First insert the operators that will be part of the cluster
        for i in 0..4 {
            let operator = dummy_operator(i);
            assert!(db.insert_operator(&operator).is_ok());
        }

        // Insert a dummy cluster
        let cluster = dummy_cluster(4);
        assert!(db.insert_cluster(cluster.clone()).is_ok());

        // Verify cluster is in memory
        assert!(db.cluster_exists(&cluster.cluster_id));
        assert_eq!(
            db.cluster_members[&cluster.cluster_id].len(),
            cluster.cluster_members.len()
        );

        // Verify cluster is in the underlying database
        let cluster_row = get_cluster_from_db(&db, cluster.cluster_id);
        assert!(cluster_row.is_some());
        let (db_cluster_id, db_faulty, db_liquidated) = cluster_row.unwrap();
        assert_eq!(db_cluster_id, *cluster.cluster_id as i64);
        assert_eq!(db_faulty, cluster.faulty as i64);
        assert_eq!(db_liquidated, cluster.liquidated);

        // Verify cluster members are in the underlying database
        for member in &cluster.cluster_members {
            let member_row = get_cluster_member_from_db(&db, member.cluster_id, member.operator_id);
            assert!(member_row.is_some());
            let (db_cluster_id, db_operator_id) = member_row.unwrap();
            assert_eq!(db_cluster_id, *member.cluster_id as i64);
            assert_eq!(db_operator_id, *member.operator_id as i64);
        }

        // Verify that the shares are in the database
        let all_shares = get_shares_from_db(&db, cluster.cluster_id);
        assert!(!all_shares.is_empty());

        // Verify that the validator is in the database
        let validator_pubkey_str = cluster.validator_metadata.validator_pubkey.to_string();
        assert!(get_validator_from_db(&db, &validator_pubkey_str).is_some());
    }

    #[test]
    /// Try inserting a cluster that does not already have registers operators in the database
    fn test_insert_cluster_without_operators() {
        // Create a temporary database
        let dir = tempdir().unwrap();
        let file = dir.path().join("db.sqlite");
        let mut db = NetworkDatabase::create(&file).unwrap();

        // Try to insert a cluster without first inserting its operators
        let cluster = dummy_cluster(4);

        // This should fail because the operators don't exist in the database
        assert!(db.insert_cluster(cluster).is_err());
    }

    #[test]
    fn test_delete_cluster() {
        // Create a temporary database
        let dir = tempdir().unwrap();
        let file = dir.path().join("db.sqlite");
        let mut db = NetworkDatabase::create(&file).unwrap();

        // populate the db with operators and cluster
        let cluster = db_with_cluster(&mut db);

        // Delete the cluster and then confirm it is gone from memory and dbb
        assert!(db.delete_cluster(cluster.cluster_id).is_ok());

        let cluster_row = get_cluster_from_db(&db, cluster.cluster_id);
        assert!(!db.cluster_exists(&cluster.cluster_id));
        assert!(cluster_row.is_none());

        // Make sure all the members are gone
        for member in &cluster.cluster_members {
            let member_row = get_cluster_member_from_db(&db, member.cluster_id, member.operator_id);
            assert!(member_row.is_none());
        }

        // Make sure all the shares are gone
        let all_shares = get_shares_from_db(&db, cluster.cluster_id);
        assert!(all_shares.is_empty());

        // Make sure the validator this cluster represented is gone
        let validator_pubkey_str = cluster.validator_metadata.validator_pubkey.to_string();
        assert!(get_validator_from_db(&db, &validator_pubkey_str).is_none());
    }
}
