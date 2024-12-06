use crate::{DatabaseError, NetworkDatabase};
use rusqlite::{params, Transaction};
use ssv_types::{Cluster, ClusterId, ClusterMember};
use types::PublicKey;

/// Implements all cluster related functionality on the database
impl NetworkDatabase {
    /// Inserts a new cluster into the database
    pub fn insert_cluster(&mut self, cluster: Cluster) -> Result<(), DatabaseError> {
        let mut conn = self.connection()?;
        let tx = conn.transaction()?;

        // Insert the top level cluster data and associated validator metadata
        tx.execute(
            "INSERT INTO clusters (cluster_id, faulty) VALUES (?1, ?2)",
            params![*cluster.cluster_id, 0],
        )?;
        tx.execute(
            "INSERT INTO validators (validator_pubkey, cluster_id) VALUES (?1, ?2)",
            params![
                cluster.validator_metadata.validator_pubkey.to_string(),
                *cluster.cluster_id
            ],
        )?;

        // Now, insert all the cluster members
        self.insert_cluster_members(
            &tx,
            &cluster.cluster_members,
            &cluster.validator_metadata.validator_pubkey,
        )?;

        // Commit all operators to the db
        tx.commit()?;

        // Since we have successfully committed, we can now store everything in memory
        self.clusters.insert(cluster.cluster_id, cluster.clone());
        for member in cluster.cluster_members {
            let key = member.share.share_pubkey.clone();
            self.shares.insert(key, member.share);
        }
        Ok(())
    }

    // Helper to insert all of the cluster members
    fn insert_cluster_members(
        &mut self,
        tx: &Transaction<'_>,
        cluster_members: &Vec<ClusterMember>,
        validator_pubkey: &PublicKey,
    ) -> Result<(), DatabaseError> {
        for member in cluster_members {
            // insert the member
            tx.execute(
                "INSERT INTO cluster_members (cluster_id, operator_id) VALUES (?1, ?2)",
                params![*member.cluster_id, *member.operator_id],
            )?;

            // insert the members share
            self.insert_share(
                tx,
                &member.share,
                member.cluster_id,
                member.operator_id,
                validator_pubkey,
            )?;
        }
        Ok(())
    }

    /// Delete a cluster from the database. This will cascade and delete all corresponding cluster
    /// members, shares, and validator metadata
    /// This corresponds to a validator being removed or exiting
    pub fn delete_cluster(&mut self, id: ClusterId) -> Result<(), DatabaseError> {
        // make sure this cluster exists
        if !self.clusters.contains_key(&id) {
            return Ok(());
        }

        let conn = self.connection()?;
        conn.execute("DELETE FROM clusters WHERE cluster_id = ?1", params![*id])?;

        // remove all in memory stores: todo!() need to figure out exactly how to structure in
        // memory
        let cluster = self.clusters.remove(&id);
        Ok(())
    }

    /// Fetch a cluster
    pub fn get_cluster(&self, id: &ClusterId) -> Option<Cluster> {
        self.clusters.get(id).cloned()
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

        // Verify cluster can be retrieved from memory
        let retrieved = db.get_cluster(&cluster.cluster_id);
        assert!(retrieved.is_some());

        // Check to make sure the data is expected
        let retrieved = retrieved.unwrap();
        assert_eq!(retrieved.cluster_id, cluster.cluster_id);
        assert_eq!(
            retrieved.cluster_members.len(),
            cluster.cluster_members.len()
        );
        assert_eq!(retrieved.faulty, cluster.faulty);
        assert_eq!(retrieved.liquidated, cluster.liquidated);

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
        assert!(db.get_cluster(&cluster.cluster_id).is_none());
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
