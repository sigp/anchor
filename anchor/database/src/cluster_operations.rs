use crate::NetworkDatabase;
use rusqlite::{params, Transaction};
use ssv_types::{Cluster, ClusterId, ClusterMember};

/// Implements all cluster related functionality on the database
impl NetworkDatabase {
    /// Inserts a new cluster into the database
    pub fn insert_cluster(&mut self, cluster: Cluster) -> Result<(), String> {
        let mut conn = self.connection()?;
        let tx = conn
            .transaction()
            .map_err(|e| format!("Unable to start a trnsaction: {:?}", e))?;

        // Insert the top level cluster data
        tx.execute(
            "INSERT INTO clusters (cluster_id, faulty) VALUES (?1, ?2)",
            params![*cluster.cluster_id, 0],
        )
        .map_err(|e| format!("Failed to insert cluster {:?}", e))?;

        // Now, insert all the cluster members
        self.insert_cluster_members(&tx, &cluster.cluster_members)?;

        // Commit all operators to the db
        tx.commit()
            .map_err(|e| format!("Failed to commit transaction: {:?}", e))?;

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
    ) -> Result<(), String> {
        for member in cluster_members {
            // insert the member
            tx.execute(
                "INSERT INTO clusters_members (cluster_id, operator_id) VALUES (?1, ?2)",
                params![*member.cluster_id, *member.operator_id],
            )
            .map_err(|e| format!("Failed to insert cluster member {:?}", e))?;

            // insert the members share
            self.insert_share(tx, &member.share, &member.cluster_id, &member.operator_id)?;
        }
        Ok(())
    }

    // Fetch a cluster that we are in
    pub fn get_cluster(&self, id: &ClusterId) -> Option<Cluster> {
        self.clusters.get(id).cloned()
    }

    /// Checks to see if we are a member of the cluster
    pub fn member_of_cluster(&self, id: &ClusterId) -> bool {
        self.clusters.contains_key(id)
    }
}
