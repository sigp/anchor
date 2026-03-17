use std::path::PathBuf;

use rusqlite::Connection;
use tempfile::TempDir;

use crate::{
    DatabaseError, schema,
    test_utils::{generators, queries},
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NetworkDatabase;

    const TEST_NETWORK_1: &str = "testnet1";
    const TEST_NETWORK_2: &str = "testnet2";

    #[test]
    fn test_new_database_creation() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let db_path = temp_dir.path().join("test.db");

        // Ensure database is created successfully
        let result = schema::ensure_up_to_date(&db_path, TEST_NETWORK_1);
        assert!(result.is_ok(), "Failed to create new database: {result:?}",);

        // Verify database file was created
        assert!(db_path.exists(), "Database file should exist");

        // Verify metadata table contains correct initial values
        let conn = Connection::open(&db_path).expect("Failed to open database");
        let metadata = queries::get_metadata(&conn).expect("Failed to get metadata");

        assert_eq!(
            metadata.schema_version, 4,
            "Initial schema version should be 4"
        );
        assert_eq!(
            metadata.network_name, TEST_NETWORK_1,
            "Network name should match input"
        );
        assert_eq!(metadata.block_number, 0, "Initial block number should be 0");
    }

    #[test]
    fn test_network_name_validation() {
        // Uses file-based DB to test reopening with different network
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let db_path = temp_dir.path().join("test.db");

        // Create database with first network
        schema::ensure_up_to_date(&db_path, TEST_NETWORK_1).expect("Failed to create database");

        // Try to open with different network - should fail
        let result = schema::ensure_up_to_date(&db_path, TEST_NETWORK_2);
        assert!(result.is_err(), "Should fail with incorrect network");

        match result.unwrap_err() {
            DatabaseError::AlreadyPresent(msg) => {
                assert!(
                    msg.contains(TEST_NETWORK_1) && msg.contains(TEST_NETWORK_2),
                    "Error should mention both networks: {msg}"
                );
            }
            other => panic!("Expected AlreadyPresent error, got: {other:?}"),
        }
    }

    #[test]
    fn test_network_name_validation_success() {
        // Uses file-based DB to test reopening with same network
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let db_path = temp_dir.path().join("test.db");

        // Create database with network
        schema::ensure_up_to_date(&db_path, TEST_NETWORK_1).expect("Failed to create database");

        // Open with same network - should succeed
        let result = schema::ensure_up_to_date(&db_path, TEST_NETWORK_1);
        assert!(result.is_ok(), "Should succeed with correct network");
    }

    #[test]
    fn test_unknown_database_rejection() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let db_path = temp_dir.path().join("test.db");

        // Create a completely unknown database
        create_unknown_database(&db_path);

        // Try to open - should fail
        let result = schema::ensure_up_to_date(&db_path, TEST_NETWORK_1);
        assert!(result.is_err(), "Should reject unknown database");

        match result.unwrap_err() {
            DatabaseError::AlreadyPresent(msg) => {
                assert!(
                    msg.contains("Unknown database schema"),
                    "Should mention unknown schema"
                );
            }
            other => panic!("Expected AlreadyPresent error, got: {other:?}"),
        }
    }

    #[test]
    fn test_future_schema_version() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let db_path = temp_dir.path().join("test.db");

        // Create database with future schema version
        create_future_schema_database(&db_path, TEST_NETWORK_1);

        // Try to open - should fail
        let result = schema::ensure_up_to_date(&db_path, TEST_NETWORK_1);
        assert!(result.is_err(), "Should reject future schema version");

        match result.unwrap_err() {
            DatabaseError::AlreadyPresent(msg) => {
                assert!(
                    msg.contains("newer than supported"),
                    "Should mention newer version"
                );
            }
            other => panic!("Expected AlreadyPresent error, got: {other:?}"),
        }
    }

    #[test]
    fn test_block_number_operations() {
        let pubkey = generators::pubkey::random_rsa();

        // Create database
        let db = NetworkDatabase::new_in_memory(&pubkey, TEST_NETWORK_1)
            .expect("Failed to create database");

        // Test initial block number
        let initial_block = db.state().get_last_processed_block();
        assert_eq!(initial_block, 0, "Initial block should be 0");

        // Update block number
        let new_block = 12345u64;
        let mut conn = db.connection().expect("Failed to get connection");
        let tx = conn.transaction().expect("Failed to start transaction");
        db.processed_block(new_block, &tx)
            .expect("Failed to update block");
        tx.commit().expect("Failed to commit transaction");

        // Verify update
        let updated_block = db.state().get_last_processed_block();
        assert_eq!(updated_block, new_block, "Block number should be updated");
    }

    #[test]
    fn test_database_outdated() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let db_path = temp_dir.path().join("test.db");

        // Create legacy database
        create_legacy_database(&db_path);

        // Ensure up to date - should error
        let err = schema::ensure_up_to_date(&db_path, TEST_NETWORK_1)
            .expect_err("Failed to detect outdated database");

        assert!(
            err.to_string().contains("outdated"),
            "Error should mention outdated database"
        );
    }

    #[test]
    fn test_migration_v1_to_v4() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let db_path = temp_dir.path().join("test.db");

        // Create a v1 database (without max_operator_id_seen column or network_name)
        create_v1_database(&db_path);

        // Verify it's version 1
        {
            let conn = Connection::open(&db_path).expect("Failed to open database");
            let version: u64 = conn
                .query_row("SELECT schema_version FROM metadata", [], |row| row.get(0))
                .expect("Failed to get schema version");
            assert_eq!(version, 1, "Should start at version 1");
        }

        // Run migration
        schema::ensure_up_to_date(&db_path, TEST_NETWORK_1).expect("Migration should succeed");

        // Verify migration succeeded
        {
            let conn = Connection::open(&db_path).expect("Failed to open database");
            let metadata = queries::get_metadata(&conn).expect("Failed to get metadata");
            assert_eq!(
                metadata.schema_version, 4,
                "Should be upgraded to version 4"
            );

            // Verify the network_name column was set
            assert_eq!(
                metadata.network_name, TEST_NETWORK_1,
                "network_name should be set after migration"
            );

            // Verify max_operator_id_seen column exists
            let max_operator_id: Option<u64> = conn
                .query_row("SELECT max_operator_id_seen FROM metadata", [], |row| {
                    row.get(0)
                })
                .expect("Failed to query max_operator_id_seen");
            assert_eq!(
                max_operator_id, None,
                "max_operator_id_seen should be NULL after migration"
            );
        }
    }

    #[test]
    fn test_migration_v2_to_v4() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let db_path = temp_dir.path().join("test.db");

        // Create a v2 database (with max_operator_id_seen but without network_name)
        create_v2_database(&db_path);

        // Verify it's version 2
        {
            let conn = Connection::open(&db_path).expect("Failed to open database");
            let version: u64 = conn
                .query_row("SELECT schema_version FROM metadata", [], |row| row.get(0))
                .expect("Failed to get schema version");
            assert_eq!(version, 2, "Should start at version 2");
        }

        // Run migration
        schema::ensure_up_to_date(&db_path, TEST_NETWORK_1).expect("Migration should succeed");

        // Verify migration succeeded
        {
            let conn = Connection::open(&db_path).expect("Failed to open database");
            let metadata = queries::get_metadata(&conn).expect("Failed to get metadata");
            assert_eq!(
                metadata.schema_version, 4,
                "Should be upgraded to version 4"
            );

            // Verify the network_name column was set
            assert_eq!(
                metadata.network_name, TEST_NETWORK_1,
                "network_name should be set after migration"
            );
        }
    }

    #[test]
    fn test_migration_v3_to_v4() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let db_path = temp_dir.path().join("test.db");

        // Create a v3 database (with network_name but without cursor columns)
        create_v3_database(&db_path, TEST_NETWORK_1);

        // Verify it's version 3
        {
            let conn = Connection::open(&db_path).expect("Failed to open database");
            let version: u64 = conn
                .query_row("SELECT schema_version FROM metadata", [], |row| row.get(0))
                .expect("Failed to get schema version");
            assert_eq!(version, 3, "Should start at version 3");
        }

        // Run migration
        schema::ensure_up_to_date(&db_path, TEST_NETWORK_1).expect("Migration should succeed");

        // Verify migration succeeded
        {
            let conn = Connection::open(&db_path).expect("Failed to open database");
            let metadata = queries::get_metadata(&conn).expect("Failed to get metadata");
            assert_eq!(
                metadata.schema_version, 4,
                "Should be upgraded to version 4"
            );

            // Verify cursor columns exist and are NULL
            let cursor: (Option<u64>, Option<u64>, Option<u64>) = conn
                .query_row(
                    "SELECT cursor_block_number, cursor_transaction_index, cursor_log_index FROM metadata",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .expect("Failed to query cursor columns");
            assert_eq!(
                cursor,
                (None, None, None),
                "Cursor columns should be NULL after migration"
            );
        }
    }

    // Helper functions for creating test databases
    fn create_v1_database(db_path: &PathBuf) {
        let conn = Connection::open(db_path).expect("Failed to create v1 database");

        // Create metadata table as it was in version 1 (without max_operator_id_seen or
        // network_name)
        conn.execute(
            "CREATE TABLE metadata (
                schema_version INTEGER NOT NULL DEFAULT 1,
                domain_type INTEGER NOT NULL,
                block_number INTEGER NOT NULL DEFAULT 0 CHECK (block_number >= 0)
            )",
            [],
        )
        .expect("Failed to create v1 metadata table");

        conn.execute("INSERT INTO metadata (domain_type) VALUES (0)", [])
            .expect("Failed to insert v1 metadata");
    }

    fn create_v2_database(db_path: &PathBuf) {
        let conn = Connection::open(db_path).expect("Failed to create v2 database");

        // Create metadata table as it was in version 2 (with max_operator_id_seen but without
        // network_name)
        conn.execute(
            "CREATE TABLE metadata (
                schema_version INTEGER NOT NULL DEFAULT 2,
                domain_type INTEGER NOT NULL,
                block_number INTEGER NOT NULL DEFAULT 0 CHECK (block_number >= 0),
                max_operator_id_seen INTEGER DEFAULT 0
            )",
            [],
        )
        .expect("Failed to create v2 metadata table");

        conn.execute("INSERT INTO metadata (domain_type) VALUES (0)", [])
            .expect("Failed to insert v2 metadata");
    }

    fn create_v3_database(db_path: &PathBuf, network_name: &str) {
        let conn = Connection::open(db_path).expect("Failed to create v3 database");

        // Create metadata table as it was in version 3 (with network_name but without cursor
        // columns)
        conn.execute(
            "CREATE TABLE metadata (
                schema_version INTEGER NOT NULL DEFAULT 3,
                domain_type INTEGER NOT NULL,
                network_name TEXT,
                block_number INTEGER NOT NULL DEFAULT 0 CHECK (block_number >= 0),
                max_operator_id_seen INTEGER DEFAULT 0
            )",
            [],
        )
        .expect("Failed to create v3 metadata table");

        conn.execute(
            "INSERT INTO metadata (domain_type, network_name) VALUES (0, ?1)",
            [network_name],
        )
        .expect("Failed to insert v3 metadata");
    }

    fn create_legacy_database(db_path: &PathBuf) {
        let conn = Connection::open(db_path).expect("Failed to create legacy database");

        // Create the old block table (without metadata)
        conn.execute(
            "CREATE TABLE block (block_number INTEGER NOT NULL DEFAULT 0)",
            [],
        )
        .expect("Failed to create legacy block table");

        conn.execute("INSERT INTO block (block_number) VALUES (42)", [])
            .expect("Failed to insert legacy block");
    }

    fn create_unknown_database(db_path: &PathBuf) {
        let conn = Connection::open(db_path).expect("Failed to create unknown database");

        // Create some random table that doesn't match our schema
        conn.execute(
            "CREATE TABLE unknown_table (id INTEGER PRIMARY KEY, data TEXT)",
            [],
        )
        .expect("Failed to create unknown table");
    }

    fn create_future_schema_database(db_path: &PathBuf, network_name: &str) {
        let conn = Connection::open(db_path).expect("Failed to create future schema database");

        // Create metadata table with future version
        conn.execute(
            "CREATE TABLE metadata (
                schema_version INTEGER NOT NULL DEFAULT 999,
                domain_type INTEGER NOT NULL,
                network_name TEXT,
                block_number INTEGER NOT NULL DEFAULT 0
            )",
            [],
        )
        .expect("Failed to create future metadata table");

        conn.execute(
            "INSERT INTO metadata (schema_version, domain_type, network_name) VALUES (999, 0, ?1)",
            [network_name],
        )
        .expect("Failed to insert future metadata");
    }
}
