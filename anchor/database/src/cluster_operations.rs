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

        // Insert the top level cluster data
        tx.execute(
            "INSERT INTO clusters (cluster_id, faulty) VALUES (?1, ?2)",
            params![*cluster.cluster_id, 0],
        )?;

        // Insert the validator metadata for the cluster
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

        // Since we have committed, we can now store everything in memory and know it will be
        // consistent
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

    // Fetch a cluster that we are in
    pub fn get_cluster(&self, id: &ClusterId) -> Option<Cluster> {
        self.clusters.get(id).cloned()
    }
}

#[cfg(test)]
mod cluster_database_tests {
    use super::*;
    use crate::test_utils::{
        dummy_cluster, dummy_operator, get_cluster_from_db, get_cluster_member_from_db,
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
    }

    #[test]
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
}
