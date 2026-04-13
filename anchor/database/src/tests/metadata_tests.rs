use std::path::Path;

use refinery::{Runner, Target};
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
    const V1_SCHEMA_VERSION: i32 = 1;
    const V2_SCHEMA_VERSION: i32 = 2;
    const V3_SCHEMA_VERSION: i32 = 3;
    const V4_SCHEMA_VERSION: i32 = 4;
    const SEEDED_BLOCK_NUMBER: u64 = 42;
    const SEEDED_MAX_OPERATOR_ID: u64 = 777;

    #[test]
    fn test_new_database_creation() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let db_path = temp_dir.path().join("test.db");

        let result = schema::ensure_up_to_date(&db_path, TEST_NETWORK_1);
        assert!(result.is_ok(), "Failed to create new database: {result:?}");
        assert!(db_path.exists(), "Database file should exist");

        let conn = Connection::open(&db_path).expect("Failed to open database");
        let metadata = queries::get_metadata(&conn).expect("Failed to get metadata");

        assert_eq!(metadata.schema_version, V4_SCHEMA_VERSION as u64);
        assert_eq!(metadata.network_name, TEST_NETWORK_1);
        assert_eq!(metadata.block_number, 0);
        assert_eq!(
            get_applied_migration_versions(&conn),
            vec![
                V1_SCHEMA_VERSION,
                V2_SCHEMA_VERSION,
                V3_SCHEMA_VERSION,
                V4_SCHEMA_VERSION,
            ]
        );
    }

    #[test]
    fn test_network_name_validation() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let db_path = temp_dir.path().join("test.db");

        schema::ensure_up_to_date(&db_path, TEST_NETWORK_1).expect("Failed to create database");

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
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let db_path = temp_dir.path().join("test.db");

        schema::ensure_up_to_date(&db_path, TEST_NETWORK_1).expect("Failed to create database");

        let result = schema::ensure_up_to_date(&db_path, TEST_NETWORK_1);
        assert!(result.is_ok(), "Should succeed with correct network");
    }

    #[test]
    fn test_unknown_database_rejection() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let db_path = temp_dir.path().join("test.db");

        create_unknown_database(&db_path);

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

        create_future_schema_database(&db_path, TEST_NETWORK_1);

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
        let db = NetworkDatabase::new_in_memory(&pubkey, TEST_NETWORK_1)
            .expect("Failed to create database");

        let initial_block = db.state().get_last_processed_block();
        assert_eq!(initial_block, 0, "Initial block should be 0");

        let new_block = 12345u64;
        let mut conn = db.connection().expect("Failed to get connection");
        let tx = conn.transaction().expect("Failed to start transaction");
        let mut pending = PendingStateUpdates::default();
        db.processed_block_tx(new_block, &tx, &mut pending)
            .expect("Failed to update block");
        commit_and_publish(&db, tx, pending);

        let updated_block = db.state().get_last_processed_block();
        assert_eq!(updated_block, new_block, "Block number should be updated");
    }

    #[test]
    fn test_database_outdated() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let db_path = temp_dir.path().join("test.db");

        create_legacy_database(&db_path);

        let err = schema::ensure_up_to_date(&db_path, TEST_NETWORK_1)
            .expect_err("Failed to detect outdated database");

        assert!(
            err.to_string().contains("outdated"),
            "Error should mention outdated database"
        );
    }

    #[test]
    fn test_migration_v1_to_v4_matches_fresh_schema() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let db_path = temp_dir.path().join("test.db");
        let fresh_db_path = temp_dir.path().join("fresh.db");

        create_supported_anchor_database(&db_path, V1_SCHEMA_VERSION, None, None);
        schema::ensure_up_to_date(&db_path, TEST_NETWORK_1).expect("Migration should succeed");
        schema::ensure_up_to_date(&fresh_db_path, TEST_NETWORK_1)
            .expect("Fresh schema creation should succeed");

        let conn = Connection::open(&db_path).expect("Failed to open migrated database");
        let fresh_conn = Connection::open(&fresh_db_path).expect("Failed to open fresh database");
        let metadata = queries::get_metadata(&conn).expect("Failed to get metadata");

        assert_eq!(metadata.schema_version, V4_SCHEMA_VERSION as u64);
        assert_eq!(metadata.network_name, TEST_NETWORK_1);
        assert_eq!(metadata.block_number, SEEDED_BLOCK_NUMBER);
        assert_eq!(get_metadata_max_operator_id_seen(&conn), Some(0));
        assert_eq!(schema_signature(&conn), schema_signature(&fresh_conn));
        assert_eq!(
            get_applied_migration_versions(&conn),
            vec![
                V1_SCHEMA_VERSION,
                V2_SCHEMA_VERSION,
                V3_SCHEMA_VERSION,
                V4_SCHEMA_VERSION,
            ]
        );
    }

    #[test]
    fn test_migration_v2_to_v4_matches_fresh_schema() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let db_path = temp_dir.path().join("test.db");
        let fresh_db_path = temp_dir.path().join("fresh.db");

        create_supported_anchor_database(
            &db_path,
            V2_SCHEMA_VERSION,
            None,
            Some(SEEDED_MAX_OPERATOR_ID),
        );
        schema::ensure_up_to_date(&db_path, TEST_NETWORK_1).expect("Migration should succeed");
        schema::ensure_up_to_date(&fresh_db_path, TEST_NETWORK_1)
            .expect("Fresh schema creation should succeed");

        let conn = Connection::open(&db_path).expect("Failed to open migrated database");
        let fresh_conn = Connection::open(&fresh_db_path).expect("Failed to open fresh database");
        let metadata = queries::get_metadata(&conn).expect("Failed to get metadata");

        assert_eq!(metadata.schema_version, V4_SCHEMA_VERSION as u64);
        assert_eq!(metadata.network_name, TEST_NETWORK_1);
        assert_eq!(metadata.block_number, SEEDED_BLOCK_NUMBER);
        assert_eq!(
            get_metadata_max_operator_id_seen(&conn),
            Some(SEEDED_MAX_OPERATOR_ID)
        );
        assert_eq!(schema_signature(&conn), schema_signature(&fresh_conn));
        assert_eq!(
            get_applied_migration_versions(&conn),
            vec![
                V1_SCHEMA_VERSION,
                V2_SCHEMA_VERSION,
                V3_SCHEMA_VERSION,
                V4_SCHEMA_VERSION,
            ]
        );
    }

    #[test]
    fn test_migration_v3_to_v4_bootstraps_refinery_history_and_matches_fresh_schema() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let db_path = temp_dir.path().join("test.db");
        let fresh_db_path = temp_dir.path().join("fresh.db");

        create_supported_anchor_database(
            &db_path,
            V3_SCHEMA_VERSION,
            Some(TEST_NETWORK_1),
            Some(SEEDED_MAX_OPERATOR_ID),
        );
        schema::ensure_up_to_date(&db_path, TEST_NETWORK_1).expect("Migration should succeed");
        schema::ensure_up_to_date(&fresh_db_path, TEST_NETWORK_1)
            .expect("Fresh schema creation should succeed");

        let conn = Connection::open(&db_path).expect("Failed to open migrated database");
        let fresh_conn = Connection::open(&fresh_db_path).expect("Failed to open fresh database");
        let metadata = queries::get_metadata(&conn).expect("Failed to get metadata");

        assert_eq!(metadata.schema_version, V4_SCHEMA_VERSION as u64);
        assert_eq!(metadata.network_name, TEST_NETWORK_1);
        assert_eq!(metadata.block_number, SEEDED_BLOCK_NUMBER);
        assert_eq!(
            get_metadata_max_operator_id_seen(&conn),
            Some(SEEDED_MAX_OPERATOR_ID)
        );
        assert_eq!(schema_signature(&conn), schema_signature(&fresh_conn));
        assert_eq!(
            get_applied_migration_versions(&conn),
            vec![
                V1_SCHEMA_VERSION,
                V2_SCHEMA_VERSION,
                V3_SCHEMA_VERSION,
                V4_SCHEMA_VERSION,
            ]
        );
    }

    #[test]
    fn test_refinery_can_ignore_missing_old_migrations_after_baseline_cutoff() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let db_path = temp_dir.path().join("test.db");

        schema::ensure_up_to_date(&db_path, TEST_NETWORK_1)
            .expect("Initial migration should succeed");

        let mut conn = Connection::open(&db_path).expect("Failed to open migrated database");
        let strict_result = reduced_migration_runner().run(&mut conn);
        assert!(
            strict_result.is_err(),
            "Missing historical migrations should fail with strict defaults"
        );

        reduced_migration_runner()
            .set_abort_missing(false)
            .run(&mut conn)
            .expect(
                "A reduced migration set should be accepted when missing migrations are allowed",
            );
    }

    fn create_supported_anchor_database(
        db_path: &Path,
        schema_version: i32,
        stored_network: Option<&str>,
        max_operator_id_seen: Option<u64>,
    ) {
        let mut conn = Connection::open(db_path).expect("Failed to create supported Anchor DB");
        schema::migration_runner_for_tests()
            .set_target(Target::Version(schema_version))
            .run(&mut conn)
            .expect("Failed to derive historical schema from real migrations");
        conn.execute_batch("DROP TABLE refinery_schema_history;")
            .expect("Failed to remove refinery history from historical test DB");

        match schema_version {
            V1_SCHEMA_VERSION => {
                conn.execute(
                    "INSERT INTO metadata (schema_version, domain_type, block_number) VALUES (?1, 0, ?2)",
                    params![schema_version, SEEDED_BLOCK_NUMBER],
                )
                .expect("Failed to insert v1 metadata");
            }
            V2_SCHEMA_VERSION => {
                conn.execute(
                    "INSERT INTO metadata (schema_version, domain_type, block_number, max_operator_id_seen) VALUES (?1, 0, ?2, ?3)",
                    params![
                        schema_version,
                        SEEDED_BLOCK_NUMBER,
                        max_operator_id_seen.expect("v2 metadata should include max_operator_id_seen")
                    ],
                )
                .expect("Failed to insert v2 metadata");
            }
            V3_SCHEMA_VERSION => {
                conn.execute(
                    "INSERT INTO metadata (schema_version, domain_type, network_name, block_number, max_operator_id_seen) VALUES (?1, 0, ?2, ?3, ?4)",
                    params![
                        schema_version,
                        stored_network.expect("v3 metadata should include network_name"),
                        SEEDED_BLOCK_NUMBER,
                        max_operator_id_seen.expect("v3 metadata should include max_operator_id_seen")
                    ],
                )
                .expect("Failed to insert v3 metadata");
            }
            other => panic!("unsupported historical schema version for test setup: {other}"),
        }
    }

    fn reduced_migration_runner() -> Runner {
        let reduced_migrations = schema::migration_runner_for_tests()
            .get_migrations()
            .iter()
            .filter(|migration| migration.version() >= V2_SCHEMA_VERSION)
            .cloned()
            .collect::<Vec<_>>();

        Runner::new(&reduced_migrations).set_grouped(true)
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

    fn create_future_schema_database(db_path: &Path, network_name: &str) {
        let conn = Connection::open(db_path).expect("Failed to create future schema database");
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

    fn get_metadata_max_operator_id_seen(conn: &Connection) -> Option<u64> {
        conn.query_row("SELECT max_operator_id_seen FROM metadata", [], |row| {
            row.get(0)
        })
        .expect("Failed to query max_operator_id_seen")
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

    fn schema_signature(conn: &Connection) -> Vec<(String, String, String, String)> {
        let mut stmt = conn
            .prepare(
                "SELECT type, name, tbl_name, COALESCE(sql, '') \
                 FROM sqlite_master \
                 WHERE name NOT LIKE 'sqlite_%' \
                 ORDER BY type, name",
            )
            .expect("Failed to prepare schema introspection query");
        stmt.query_map([], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                normalize_sql(&row.get::<_, String>(3)?),
            ))
        })
        .expect("Failed to introspect schema")
        .map(|row| row.expect("Failed to parse schema row"))
        .collect()
    }

    fn normalize_sql(sql: &str) -> String {
        sql.split_whitespace().collect::<Vec<_>>().join(" ")
    }
}
