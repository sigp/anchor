use super::test_prelude::*;

#[cfg(test)]
mod cluster_database_tests {
    use super::*;

    #[test]
    // Test inserting a cluster into the database
    fn test_insert_retrieve_cluster() {
        let fixture = TestFixture::new();
        assertions::assert_cluster_exists_in_db(&fixture.db, &fixture.cluster);
        assertions::assert_cluster_exists_in_store(&fixture.db, &fixture.cluster);
    }

    #[test]
    // Try inserting a cluster that does not already have registers operators in the database
    fn test_insert_cluster_without_operators() {
        let fixture = TestFixture::new_empty();
        let cluster = generators::cluster::random(3);
        fixture
            .db
            .insert_cluster(cluster)
            .expect_err("Insertion should fail");
    }

    #[test]
    // Test deleting a cluster and make sure that it is properly cleaned up
    fn test_delete_cluster() {
        let fixture = TestFixture::new();
        fixture
            .db
            .delete_cluster(fixture.cluster.cluster_id)
            .expect("Failed to delete cluster");
        assertions::assert_cluster_exists_not_in_db(&fixture.db, &fixture.cluster);
        assertions::assert_cluster_exists_not_in_store(&fixture.db, &fixture.cluster);
    }

    #[test]
    // Test updating the operational status of the cluster
    fn test_update_cluster_status() {
        let fixture = TestFixture::new();
        let cluster_id = fixture.cluster.cluster_id;

        // Test updating to liquidated
        fixture
            .db
            .update_status(cluster_id, true)
            .expect("Failed to update cluster status");

        // Verify both in memory and database
        let (_, _, liquidated) =
            queries::get_cluster(&fixture.db, cluster_id).expect("Cluster not found");
        assert!(liquidated, "Cluster should be liquidated");
    }

    #[test]
    // Test inserting two clusters that an operator is a member of
    fn test_insert_two_clusters() {
        let fixture = TestFixture::new_empty();
        let us_pubkey = fixture.pubkey;
        let us_operator = generators::operator::with_pubkey(us_pubkey);

        //generate a few more operators then add us into the group
        let mut operators: Vec<Operator> = (0..3).map(generators::operator::with_id).collect();
        operators.push(us_operator);

        // inset all of the operators
        for op in &operators {
            fixture
                .db
                .insert_operator(op)
                .expect("Failed to insert operator");
        }

        // generate and insert 2 clusters
        let cluster1 = generators::cluster::with_operators(&operators);
        let cluster2 = generators::cluster::with_operators(&operators);
        for c in [cluster1.clone(), cluster2.clone()] {
            fixture
                .db
                .insert_cluster(c)
                .expect("Failed to insert cluster");
        }

        // make sure they are in the db and state store is expected
        assertions::assert_cluster_exists_in_db(&fixture.db, &cluster1);
        assertions::assert_cluster_exists_in_db(&fixture.db, &cluster2);
        assertions::assert_cluster_exists_in_store(&fixture.db, &cluster1);
        assertions::assert_cluster_exists_in_store(&fixture.db, &cluster2);
    }

    #[test]
    // Test inserting a cluster that already exists
    fn test_duplicate_cluster_insert() {
        let fixture = TestFixture::new();
        fixture
            .db
            .insert_cluster(fixture.cluster)
            .expect_err("Expected failure when inserting cluster that already exists");
    }
}
