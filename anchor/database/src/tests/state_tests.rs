use super::test_prelude::*;

#[cfg(test)]
mod state_database_tests {
    use super::*;

    #[test]
    // Make sure all of the previously inserted operators are present after restart
    fn test_operator_store() {
        // Create new test fixture with populated DB
        let mut fixture = TestFixture::new(Some(1));

        // drop the database and then recreate it
        drop(fixture.db);
        fixture.db = NetworkDatabase::new(&fixture.path, Some(OperatorId(1)))
            .expect("Failed to create database");

        // confirm that all of the operators exist were
        for operator in fixture.operators {
            assertions::assert_operator_exists_fully(&fixture.db, &operator);
        }
    }

    #[test]
    fn test_cluster_after_restart() {
        // Create new test fixture with populated DB
        let mut fixture = TestFixture::new(Some(1));
        let cluster = fixture.cluster;

        // drop the database and then recreate it
        drop(fixture.db);
        fixture.db = NetworkDatabase::new(&fixture.path, Some(OperatorId(1)))
            .expect("Failed to create database");

        // Confirm all cluster related data is still correct
        assertions::assert_cluster_exists_fully(&fixture.db, &cluster);
    }
}
