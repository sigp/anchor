use super::test_prelude::*;

#[cfg(test)]
mod cluster_database_tests {
    use super::*;

    #[test]
    // Test inserting a cluster into the database
    fn test_insert_retrieve_cluster() {
        let fixture = TestFixture::new();
        assertions::assert_cluster_exists_fully(&fixture.db, &fixture.cluster);
    }

    #[test]
    // Try inserting a cluster that does not already have registers operators in the database
    fn test_insert_cluster_without_operators() {
        let mut fixture = TestFixture::new_empty();
        let cluster = generators::cluster::random(3);
        fixture
            .db
            .insert_cluster(cluster)
            .expect_err("Insertion should fail");
    }

    #[test]
    // Test deleting a cluster and make sure that it is properly cleaned up
    fn test_delete_cluster() {
        let mut fixture = TestFixture::new();
        fixture
            .db
            .delete_cluster(fixture.cluster.cluster_id)
            .expect("Failed to delete cluster");
        assertions::assert_cluster_exists_not_fully(&fixture.db, &fixture.cluster);
    }
}
