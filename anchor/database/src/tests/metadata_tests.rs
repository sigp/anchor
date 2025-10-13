use std::path::PathBuf;

use rusqlite::Connection;
use ssv_types::domain_type::DomainType;
use tempfile::TempDir;

use super::test_prelude::*;
use crate::{DatabaseError, schema};

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_DOMAIN_1: DomainType = DomainType([42, 42, 42, 42]);
    const TEST_DOMAIN_2: DomainType = DomainType([99, 99, 99, 99]);

    #[test]
    fn test_new_database_creation() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let db_path = temp_dir.path().join("test.db");

        // Ensure database is created successfully
        let result = schema::ensure_up_to_date(&db_path, TEST_DOMAIN_1);
        assert!(result.is_ok(), "Failed to create new database: {result:?}",);

        // Verify database file was created
        assert!(db_path.exists(), "Database file should exist");

        // Verify metadata table contains correct initial values
        let conn = Connection::open(&db_path).expect("Failed to open database");
        let metadata = queries::get_metadata(&conn).expect("Failed to get metadata");

        assert_eq!(
            metadata.schema_version, 1,
            "Initial schema version should be 1"
        );
        assert_eq!(metadata.domain, TEST_DOMAIN_1, "Domain should match input");
        assert_eq!(metadata.block_number, 0, "Initial block number should be 0");
    }

    #[test]
    fn test_domain_type_validation() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let db_path = temp_dir.path().join("test.db");

        // Create database with first domain
        schema::ensure_up_to_date(&db_path, TEST_DOMAIN_1).expect("Failed to create database");

        // Try to open with different domain - should fail
        let result = schema::ensure_up_to_date(&db_path, TEST_DOMAIN_2);
        assert!(result.is_err(), "Should fail with incorrect domain");

        match result.unwrap_err() {
            DatabaseError::AlreadyPresent(msg) => {
                assert!(
                    msg.contains("different network"),
                    "Error should mention different network"
                );
            }
            other => panic!("Expected AlreadyPresent error, got: {other:?}"),
        }
    }

    #[test]
    fn test_domain_type_validation_success() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let db_path = temp_dir.path().join("test.db");

        // Create database with domain
        schema::ensure_up_to_date(&db_path, TEST_DOMAIN_1).expect("Failed to create database");

        // Open with same domain - should succeed
        let result = schema::ensure_up_to_date(&db_path, TEST_DOMAIN_1);
        assert!(result.is_ok(), "Should succeed with correct domain");
    }

    #[test]
    fn test_unknown_database_rejection() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let db_path = temp_dir.path().join("test.db");

        // Create a completely unknown database
        create_unknown_database(&db_path);

        // Try to open - should fail
        let result = schema::ensure_up_to_date(&db_path, TEST_DOMAIN_1);
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
        create_future_schema_database(&db_path, TEST_DOMAIN_1);

        // Try to open - should fail
        let result = schema::ensure_up_to_date(&db_path, TEST_DOMAIN_1);
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
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let db_path = temp_dir.path().join("test.db");
        let pubkey = generators::pubkey::random_rsa();

        // Create database
        let db = NetworkDatabase::new(&db_path, &pubkey, TEST_DOMAIN_1)
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

        // Verify persistence after restart
        drop(db);
        let db2 = NetworkDatabase::new(&db_path, &pubkey, TEST_DOMAIN_1)
            .expect("Failed to reopen database");
        let persisted_block = db2.state().get_last_processed_block();
        assert_eq!(persisted_block, new_block, "Block number should persist");
    }

    #[test]
    fn test_database_outdated() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let db_path = temp_dir.path().join("test.db");

        // Create legacy database
        create_legacy_database(&db_path);

        // Ensure up to date - should error
        let err = schema::ensure_up_to_date(&db_path, TEST_DOMAIN_1)
            .expect_err("Failed to detect outdated database");

        assert!(
            err.to_string().contains("outdated"),
            "Error should mention outdated database"
        );
    }

    #[test]
    fn test_domain_type_serialization() {
        // Test DomainType conversion to/from SQL
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let db_path = temp_dir.path().join("test.db");
        let conn = Connection::open(&db_path).expect("Failed to create database");

        // Create metadata table
        conn.execute(
            "CREATE TABLE test_metadata (domain_type INTEGER NOT NULL)",
            [],
        )
        .expect("Failed to create test table");

        // Insert domain type
        conn.execute(
            "INSERT INTO test_metadata (domain_type) VALUES (?1)",
            [&TEST_DOMAIN_1],
        )
        .expect("Failed to insert domain");

        // Read back domain type
        let retrieved_domain: DomainType = conn
            .query_row("SELECT domain_type FROM test_metadata", [], |row| {
                row.get(0)
            })
            .expect("Failed to retrieve domain");

        assert_eq!(
            retrieved_domain, TEST_DOMAIN_1,
            "Domain type should round-trip correctly"
        );
    }

    // Helper functions for creating test databases
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

    fn create_future_schema_database(db_path: &PathBuf, domain: DomainType) {
        let conn = Connection::open(db_path).expect("Failed to create future schema database");

        // Create metadata table with future version
        conn.execute(
            "CREATE TABLE metadata (
                schema_version INTEGER NOT NULL DEFAULT 999,
                domain_type INTEGER NOT NULL,
                block_number INTEGER NOT NULL DEFAULT 0
            )",
            [],
        )
        .expect("Failed to create future metadata table");

        conn.execute(
            "INSERT INTO metadata (schema_version, domain_type) VALUES (999, ?1)",
            [&domain],
        )
        .expect("Failed to insert future metadata");
    }
}
