use std::path::Path;

use rusqlite::{Connection, params};
use tempfile::TempDir;

use crate::{
    DatabaseError, schema,
    test_utils::{generators, queries},
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{NetworkDatabase, PendingStateUpdates, test_utils::commit_and_publish};

    const TEST_NETWORK_1: &str = "testnet1";
    const TEST_NETWORK_2: &str = "testnet2";
    const LATEST_SCHEMA_VERSION: u64 = 4;
    const PRODUCTION_BASELINE_MIGRATION_VERSION: i32 = 1;
    const V4_REFINERY_MIGRATION_VERSION: i32 = 2;
    const SEEDED_BLOCK_NUMBER: u64 = 42;
    const SEEDED_MAX_OPERATOR_ID: u64 = 777;

    #[test]
    fn test_new_database_creation() {
        // Arrange: create a fresh database path.
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let db_path = temp_dir.path().join("test.db");

        // Act: initialize the database through the runtime path.
        schema::ensure_up_to_date(&db_path, TEST_NETWORK_1).expect("Failed to create new database");

        // Assert: fresh DBs apply the production baseline and the first refinery migration.
        let conn = Connection::open(&db_path).expect("Failed to open database");
        let metadata = queries::get_metadata(&conn).expect("Failed to get metadata");
        assert_eq!(metadata.schema_version, LATEST_SCHEMA_VERSION);
        assert_eq!(metadata.network_name, TEST_NETWORK_1);
        assert_eq!(metadata.block_number, 0);
        assert_eq!(
            get_applied_migration_versions(&conn),
            vec![
                PRODUCTION_BASELINE_MIGRATION_VERSION,
                V4_REFINERY_MIGRATION_VERSION,
            ]
        );
        assert!(
            has_validator_index(&conn),
            "v4 migration should create validator index"
        );
    }

    #[test]
    fn test_network_name_validation() {
        // Arrange: initialize a database for one network.
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let db_path = temp_dir.path().join("test.db");
        schema::ensure_up_to_date(&db_path, TEST_NETWORK_1).expect("Failed to create database");

        // Act: reopen it with a different network name.
        let result = schema::ensure_up_to_date(&db_path, TEST_NETWORK_2);

        // Assert: runtime network validation still rejects mismatches.
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
        // Arrange: initialize a database for the target network.
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let db_path = temp_dir.path().join("test.db");
        schema::ensure_up_to_date(&db_path, TEST_NETWORK_1).expect("Failed to create database");

        // Act/Assert: reopening a refinery-managed DB for the same network succeeds.
        schema::ensure_up_to_date(&db_path, TEST_NETWORK_1)
            .expect("Should succeed with correct network");
    }

    #[test]
    fn test_manual_v3_production_baseline_adoption() {
        // Arrange: create a manual v3 DB matching the shipped fresh schema.
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let db_path = temp_dir.path().join("test.db");
        create_manual_v3_production_database(
            &db_path,
            Some(TEST_NETWORK_1),
            Some(SEEDED_MAX_OPERATOR_ID),
        );

        // Act: adopt it into refinery and run the first refinery migration.
        schema::ensure_up_to_date(&db_path, TEST_NETWORK_1).expect("Adoption should succeed");

        // Assert: data is preserved and the DB is now fully refinery-managed at schema v4.
        let conn = Connection::open(&db_path).expect("Failed to open adopted database");
        let metadata = queries::get_metadata(&conn).expect("Failed to get metadata");
        assert_eq!(metadata.schema_version, LATEST_SCHEMA_VERSION);
        assert_eq!(metadata.network_name, TEST_NETWORK_1);
        assert_eq!(metadata.block_number, SEEDED_BLOCK_NUMBER);
        assert_eq!(
            get_metadata_max_operator_id_seen(&conn),
            Some(SEEDED_MAX_OPERATOR_ID)
        );
        assert_eq!(
            get_applied_migration_versions(&conn),
            vec![
                PRODUCTION_BASELINE_MIGRATION_VERSION,
                V4_REFINERY_MIGRATION_VERSION,
            ]
        );
        assert!(
            has_validator_index(&conn),
            "v4 migration should create validator index"
        );
    }

    #[test]
    fn test_manual_v3_upgraded_shape_adoption() {
        // Arrange: create a manual v3 DB that reached v3 through older ALTER TABLE upgrades.
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let db_path = temp_dir.path().join("test.db");
        create_manual_v3_upgraded_database(&db_path, None, None);

        // Act: adopt it into refinery and run the first refinery migration.
        schema::ensure_up_to_date(&db_path, TEST_NETWORK_1).expect("Adoption should succeed");

        // Assert: the weaker metadata shape is canonicalized and then upgraded to v4.
        let conn = Connection::open(&db_path).expect("Failed to open adopted database");
        let metadata = queries::get_metadata(&conn).expect("Failed to get metadata");
        assert_eq!(metadata.schema_version, LATEST_SCHEMA_VERSION);
        assert_eq!(metadata.network_name, TEST_NETWORK_1);
        assert_eq!(metadata.block_number, SEEDED_BLOCK_NUMBER);
        assert_eq!(get_metadata_max_operator_id_seen(&conn), Some(0));
        assert_eq!(
            get_applied_migration_versions(&conn),
            vec![
                PRODUCTION_BASELINE_MIGRATION_VERSION,
                V4_REFINERY_MIGRATION_VERSION,
            ]
        );
        assert!(
            has_validator_index(&conn),
            "v4 migration should create validator index"
        );
    }

    #[test]
    fn test_legacy_database_rejection() {
        // Arrange: create a legacy pre-metadata Anchor DB.
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let db_path = temp_dir.path().join("test.db");
        create_legacy_database(&db_path);

        // Act: try to open it through the cutover path.
        let err = schema::ensure_up_to_date(&db_path, TEST_NETWORK_1)
            .expect_err("Failed to detect outdated database");

        // Assert: pre-metadata DBs are still rejected.
        assert!(
            err.to_string().contains("unsupported pre-refinery"),
            "Error should mention unsupported pre-refinery database"
        );
    }

    #[test]
    fn test_unknown_database_rejection() {
        // Arrange: create a database that does not look like Anchor.
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let db_path = temp_dir.path().join("test.db");
        create_unknown_database(&db_path);

        // Act: try to open it as an Anchor DB.
        let result = schema::ensure_up_to_date(&db_path, TEST_NETWORK_1);

        // Assert: unknown schemas are rejected.
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
    fn test_block_number_operations() {
        // Arrange: create an in-memory database and begin a transaction.
        let pubkey = generators::pubkey::random_rsa();
        let db = NetworkDatabase::new_in_memory(&pubkey, TEST_NETWORK_1)
            .expect("Failed to create database");

        let initial_block = db.state().get_last_processed_block();
        assert_eq!(initial_block, 0, "Initial block should be 0");

        let new_block = 12345u64;
        let mut conn = db.connection().expect("Failed to get connection");
        let tx = conn.transaction().expect("Failed to start transaction");
        let mut pending = PendingStateUpdates::default();

        // Act: update the processed block through the normal transaction path.
        db.processed_block_tx(new_block, &tx, &mut pending)
            .expect("Failed to update block");
        commit_and_publish(&db, tx, pending);

        // Assert: the state reflects the committed block.
        let updated_block = db.state().get_last_processed_block();
        assert_eq!(updated_block, new_block, "Block number should be updated");
    }

    fn create_manual_v3_production_database(
        db_path: &Path,
        network_name: Option<&str>,
        max_operator_id_seen: Option<u64>,
    ) {
        let conn = Connection::open(db_path).expect("Failed to create manual v3 database");
        conn.execute_batch(include_str!("../migrations/V1__production_baseline.sql"))
            .expect("Failed to create production baseline schema");
        conn.execute(
            "INSERT INTO metadata (schema_version, domain_type, network_name, block_number, max_operator_id_seen)
             VALUES (?1, 0, ?2, ?3, ?4)",
            params![
                3,
                network_name.expect("production v3 DB should have network_name"),
                SEEDED_BLOCK_NUMBER,
                max_operator_id_seen.expect("production v3 DB should have max_operator_id_seen"),
            ],
        )
        .expect("Failed to insert production v3 metadata");
    }

    fn create_manual_v3_upgraded_database(
        db_path: &Path,
        network_name: Option<&str>,
        max_operator_id_seen: Option<u64>,
    ) {
        let conn = Connection::open(db_path).expect("Failed to create upgraded v3 database");
        conn.execute_batch(include_str!("../migrations/V1__production_baseline.sql"))
            .expect("Failed to create shared v3 tables");
        conn.execute_batch(
            "DROP TRIGGER unique_metadata;
             DROP TABLE metadata;
             CREATE TABLE metadata (
                 schema_version INTEGER NOT NULL DEFAULT 1,
                 domain_type INTEGER NOT NULL,
                 block_number INTEGER NOT NULL DEFAULT 0 CHECK (block_number >= 0)
             );
             ALTER TABLE metadata ADD COLUMN max_operator_id_seen INTEGER;
             ALTER TABLE metadata ADD COLUMN network_name TEXT;
             CREATE TRIGGER unique_metadata
                 BEFORE INSERT ON metadata
                 WHEN (SELECT COUNT(*) FROM metadata) >= 1
             BEGIN
                 SELECT RAISE(FAIL, 'we can only have one metadata row');
             END;",
        )
        .expect("Failed to recreate upgraded v3 metadata");
        conn.execute(
            "INSERT INTO metadata (schema_version, domain_type, block_number, max_operator_id_seen, network_name)
             VALUES (?1, 0, ?2, ?3, ?4)",
            params![
                3,
                SEEDED_BLOCK_NUMBER,
                max_operator_id_seen,
                network_name,
            ],
        )
        .expect("Failed to insert upgraded v3 metadata");
    }

    fn create_legacy_database(db_path: &Path) {
        let conn = Connection::open(db_path).expect("Failed to create legacy database");
        conn.execute(
            "CREATE TABLE block (block_number INTEGER NOT NULL DEFAULT 0)",
            [],
        )
        .expect("Failed to create legacy block table");
        conn.execute("INSERT INTO block (block_number) VALUES (42)", [])
            .expect("Failed to insert legacy block");
    }

    fn create_unknown_database(db_path: &Path) {
        let conn = Connection::open(db_path).expect("Failed to create unknown database");
        conn.execute(
            "CREATE TABLE unknown_table (id INTEGER PRIMARY KEY, data TEXT)",
            [],
        )
        .expect("Failed to create unknown table");
    }

    fn get_applied_migration_versions(conn: &Connection) -> Vec<i32> {
        let mut stmt = conn
            .prepare("SELECT version FROM refinery_schema_history ORDER BY version")
            .expect("Failed to prepare migration history query");
        stmt.query_map([], |row| row.get(0))
            .expect("Failed to query migration history")
            .map(|row| row.expect("Failed to parse migration version"))
            .collect()
    }

    fn get_metadata_max_operator_id_seen(conn: &Connection) -> Option<u64> {
        conn.query_row("SELECT max_operator_id_seen FROM metadata", [], |row| {
            row.get(0)
        })
        .expect("Failed to query max_operator_id_seen")
    }

    fn has_validator_index(conn: &Connection) -> bool {
        conn.query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'index' AND name = 'idx_validators_validator_index'",
            [],
            |_| Ok(()),
        )
        .is_ok()
    }
}
