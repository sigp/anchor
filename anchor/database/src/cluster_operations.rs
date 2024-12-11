use super::{DatabaseError, NetworkDatabase, SqlStatement, SQL};
use rusqlite::params;
use ssv_types::{Cluster, ClusterId};

/// Implements all cluster related functionality on the database
impl NetworkDatabase {
    /// Inserts a new cluster into the database
    pub fn insert_cluster(&mut self, cluster: Cluster) -> Result<(), DatabaseError> {
        let mut conn = self.connection()?;
        let tx = conn.transaction()?;

        // Insert the top level cluster data and associated validator metadata
        tx.prepare_cached(SQL[&SqlStatement::InsertCluster])?
            .execute(params![*cluster.cluster_id])?;
        tx.prepare_cached(SQL[&SqlStatement::InsertValidator])?
            .execute(params![
                cluster.validator_metadata.validator_pubkey.to_string(),
                *cluster.cluster_id,
                cluster.validator_metadata.owner.to_string()
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

#[cfg(test)]
mod cluster_database_tests {
    use super::*;
    use crate::test_utils::{
        db_with_cluster, debug_print_db, dummy_cluster, dummy_operator, get_cluster_from_db,
        get_cluster_member_from_db, get_shares_from_db, get_validator_from_db,
    };
    use ssv_types::OperatorId;
    use tempfile::tempdir;

    #[test]
    // Test inserting a cluster into the database
    fn test_insert_retrieve_cluster() {
        // Create a temporary database
        let dir = tempdir().unwrap();
        let file = dir.path().join("db.sqlite");
        let mut db = NetworkDatabase::new(&file, Some(OperatorId(1))).unwrap();

        // First insert the operators that will be part of the cluster
        for i in 0..4 {
            let operator = dummy_operator(i);
            assert!(db.insert_operator(&operator).is_ok());
        }

        // Insert a dummy cluster
        let cluster = dummy_cluster(4);
        assert!(db.insert_cluster(cluster.clone()).is_ok());

        debug_print_db(&db);
        println!("{:#?}", db.state);

        // Verify cluster is in memory
        assert!(db.member_of_cluster(&cluster.cluster_id));
        assert_eq!(
            db.state.cluster_members[&cluster.cluster_id].len(),
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
        let mut db = NetworkDatabase::new(&file, None).unwrap();

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
        let mut db = NetworkDatabase::new(&file, Some(OperatorId(1))).unwrap();

        // populate the db with operators and cluster
        let cluster = db_with_cluster(&mut db);

        // Delete the cluster and then confirm it is gone from memory and dbb
        assert!(db.delete_cluster(cluster.cluster_id).is_ok());

        let cluster_row = get_cluster_from_db(&db, cluster.cluster_id);
        assert!(!db.member_of_cluster(&cluster.cluster_id));
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
