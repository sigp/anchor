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
    const PRODUCTION_BASELINE_MIGRATION_VERSION: i32 = 1;
    const CURRENT_SCHEMA_MIGRATION_VERSION: i32 = 2;
    const SEEDED_BLOCK_NUMBER: u64 = 42;

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
        assert_eq!(metadata.network_name, TEST_NETWORK_1);
        assert_eq!(metadata.block_number, 0);
        assert_eq!(
            get_applied_migration_versions(&conn),
            vec![
                PRODUCTION_BASELINE_MIGRATION_VERSION,
                CURRENT_SCHEMA_MIGRATION_VERSION,
            ]
        );
        assert!(
            has_validator_index(&conn),
            "the first refinery migration should create the validator index"
        );
        assert_eq!(get_metadata_max_operator_id_seen(&conn), Some(0));
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
    fn test_manual_v1_adoption() {
        // Arrange: create a real shipped manual v1 DB. This is the production state we observed
        // in the copied mainnet data dir, so the cutover should adopt it by stamping refinery V1
        // and then running the combined refinery V2 upgrade.
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let db_path = temp_dir.path().join("test.db");
        create_manual_v1_database(&db_path);

        // Act: adopt it into refinery and run the first refinery migration.
        schema::ensure_up_to_date(&db_path, TEST_NETWORK_1).expect("Adoption should succeed");

        // Assert: the runtime state is preserved, refinery history is initialized, and the
        // combined V2 migration materializes the current schema.
        let conn = Connection::open(&db_path).expect("Failed to open adopted database");
        let metadata = queries::get_metadata(&conn).expect("Failed to get metadata");
        assert_eq!(metadata.network_name, TEST_NETWORK_1);
        assert_eq!(metadata.block_number, SEEDED_BLOCK_NUMBER);
        assert_eq!(get_metadata_max_operator_id_seen(&conn), None);
        assert_eq!(
            get_applied_migration_versions(&conn),
            vec![
                PRODUCTION_BASELINE_MIGRATION_VERSION,
                CURRENT_SCHEMA_MIGRATION_VERSION,
            ]
        );
        assert!(
            has_validator_index(&conn),
            "the first refinery migration should create the validator index"
        );
        assert_skipped_operator_add_round_trip(&conn, 7);
    }

    #[test]
    fn test_stamped_v1_restart_recovers_and_applies_v2() {
        // Arrange: create the exact restart state after the bridge stamps V1 into
        // `refinery_schema_history` but before the later runtime call applies V2.
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let db_path = temp_dir.path().join("test.db");
        create_manual_v1_database(&db_path);
        let mut conn =
            Connection::open(&db_path).expect("Failed to open stamped v1 database for setup");
        schema::stamp_baseline_for_tests(&mut conn).expect("Failed to stamp refinery baseline");
        drop(conn);

        // Act: restart through the normal cutover path.
        schema::ensure_up_to_date(&db_path, TEST_NETWORK_1)
            .expect("Restart should recover and apply V2");

        // Assert: the retry path applies V2 and initializes the runtime metadata fields.
        let conn = Connection::open(&db_path).expect("Failed to reopen recovered database");
        let metadata = queries::get_metadata(&conn).expect("Failed to get metadata");
        assert_eq!(metadata.network_name, TEST_NETWORK_1);
        assert_eq!(metadata.block_number, SEEDED_BLOCK_NUMBER);
        assert_eq!(get_metadata_max_operator_id_seen(&conn), None);
        assert_eq!(
            get_applied_migration_versions(&conn),
            vec![
                PRODUCTION_BASELINE_MIGRATION_VERSION,
                CURRENT_SCHEMA_MIGRATION_VERSION,
            ]
        );
        assert!(
            has_validator_index(&conn),
            "the resumed cutover should still create the validator index"
        );
        assert_skipped_operator_add_round_trip(&conn, 8);
    }

    #[test]
    fn test_refinery_managed_restart_without_metadata_row_recovers() {
        // Arrange: create the partial fresh-DB state after refinery has applied V1/V2 and written
        // `refinery_schema_history`, but before `ensure_metadata_row()` has inserted the singleton
        // metadata row.
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let db_path = temp_dir.path().join("test.db");
        let mut conn = Connection::open(&db_path)
            .expect("Failed to open partial refinery-managed database for setup");
        schema::run_migrations_for_tests(&mut conn).expect("Failed to apply refinery migrations");
        drop(conn);

        // Act: restart through the normal runtime path.
        schema::ensure_up_to_date(&db_path, TEST_NETWORK_1)
            .expect("Restart should recover and insert metadata row");

        // Assert: the metadata row is created and the database is fully initialized.
        let conn = Connection::open(&db_path).expect("Failed to reopen recovered database");
        let metadata = queries::get_metadata(&conn).expect("Failed to get metadata");
        assert_eq!(metadata.network_name, TEST_NETWORK_1);
        assert_eq!(metadata.block_number, 0);
        assert_eq!(get_metadata_max_operator_id_seen(&conn), Some(0));
        assert_eq!(
            get_applied_migration_versions(&conn),
            vec![
                PRODUCTION_BASELINE_MIGRATION_VERSION,
                CURRENT_SCHEMA_MIGRATION_VERSION,
            ]
        );
        assert!(
            has_validator_index(&conn),
            "the recovered fresh DB should still have the validator index"
        );
    }

    #[test]
    fn test_unsupported_manual_schema_version_rejection() {
        // Arrange: create a pre-refinery Anchor DB that looks like metadata-backed Anchor, but
        // carries an unsupported manual schema version. The cutover only adopts the real shipped
        // schema-v1 layout.
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let db_path = temp_dir.path().join("test.db");
        create_manual_database_with_schema_version(&db_path, 2);

        // Act: try to open it through the cutover path.
        let err = schema::ensure_up_to_date(&db_path, TEST_NETWORK_1)
            .expect_err("Unsupported manual schema version should be rejected");

        // Assert: manual schemas newer than the shipped baseline are rejected explicitly.
        assert!(
            err.to_string().contains("unsupported pre-refinery"),
            "Error should mention unsupported pre-refinery database"
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

    fn create_manual_v1_database(db_path: &Path) {
        create_manual_database_with_schema_version(db_path, 1);
    }

    fn create_manual_database_with_schema_version(db_path: &Path, schema_version: u32) {
        let conn = Connection::open(db_path).expect("Failed to create manual v1 database");
        conn.execute_batch(include_str!("../migrations/V1__production_baseline.sql"))
            .expect("Failed to create production baseline schema");
        // Seed the metadata row exactly the way shipped schema-v1 databases look on disk. There
        // is no network_name yet; the cutover fills that after V2 runs. The generic
        // `schema_version` parameter lets us reuse the same physical schema for rejection tests
        // that model unsupported manual versions.
        conn.execute(
            "INSERT INTO metadata (schema_version, domain_type, block_number)
             VALUES (?1, ?2, ?3)",
            params![schema_version, 0, SEEDED_BLOCK_NUMBER],
        )
        .expect("Failed to insert manual v1 metadata");
    }

    fn create_legacy_database(db_path: &Path) {
        let conn = Connection::open(db_path).expect("Failed to create legacy database");
        // Older pre-metadata Anchor DBs were detected by the singleton `block` table rather than a
        // `metadata` row. Keep this fixture minimal so the rejection path stays obvious.
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
        // This intentionally looks like a random SQLite file with no Anchor markers so the
        // database-type classifier has to fall through to `Unknown`.
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

    fn assert_skipped_operator_add_round_trip(conn: &Connection, operator_id: u64) {
        let expected_reason = "migration coverage";

        conn.execute(
            "INSERT INTO skipped_operator_adds (operator_id, reason) VALUES (?1, ?2)",
            params![operator_id, expected_reason],
        )
        .expect("Failed to insert skipped operator marker");

        let actual_reason: String = conn
            .query_row(
                "SELECT reason FROM skipped_operator_adds WHERE operator_id = ?1",
                params![operator_id],
                |row| row.get(0),
            )
            .expect("Failed to read skipped operator marker");

        assert_eq!(
            actual_reason, expected_reason,
            "the V2 migration should materialize a writable skipped_operator_adds table"
        );
    }
}
