use super::test_prelude::*;

#[cfg(test)]
mod state_database_tests {
    use super::*;

    #[test]
    fn test_state_after_restart() {
        // Create a temporary database
        let dir = tempdir().unwrap();
        let file = dir.path().join("db.sqlite");
        let mut db = NetworkDatabase::new(&file, Some(OperatorId(1))).unwrap();

        // Insert the operators and a cluster we are a part of
        for i in 0..4 {
            let operator = dummy_operator(i);
            assert!(db.insert_operator(&operator).is_ok());
        }
        // Insert a dummy cluster
        let cluster = dummy_cluster(4);
        assert!(db.insert_cluster(cluster.clone()).is_ok());
        println!("{:#?}", db.state);

        // drop db and recreate it, stores should be built since db already exists
        drop(db);

        let db = NetworkDatabase::new(&file, Some(OperatorId(1))).unwrap();
        println!("{:#?}", db.state);
    }
}
