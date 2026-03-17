use std::path::Path;

use rusqlite::{Connection, types::Value};

use crate::{DatabaseError, sql_operations};

type SchemaVersion = u32;

/// Migration from schema version 1 to 2: Add max_operator_id_seen column to metadata table
const MIGRATION_V1_TO_V2: &str = r#"
    ALTER TABLE metadata ADD COLUMN max_operator_id_seen INTEGER;
    UPDATE metadata SET schema_version = 2;
"#;

/// Migration from schema version 2 to 3: Add network_name column to metadata table.
///
/// This replaces domain_type-based network isolation with network name.
/// The domain_type column is NOT removed because:
/// 1. SQLite doesn't support DROP COLUMN easily (requires table recreation)
/// 2. Keeping it maintains backwards compatibility
/// 3. It's harmless as dead data - we simply ignore it
///
/// The domain_type was problematic because it changes at each fork activation,
/// causing "database for different network" errors after fork transitions.
/// Network name (e.g., "mainnet", "hoodi") is stable across forks.
const MIGRATION_V2_TO_V3: &str = r#"
    ALTER TABLE metadata ADD COLUMN network_name TEXT;
    UPDATE metadata SET schema_version = 3;
"#;

/// Migration from schema version 3 to 4: Track the exact processed log position inside a block.
const MIGRATION_V3_TO_V4: &str = r#"
    ALTER TABLE metadata ADD COLUMN cursor_block_number INTEGER;
    ALTER TABLE metadata ADD COLUMN cursor_transaction_index INTEGER;
    ALTER TABLE metadata ADD COLUMN cursor_log_index INTEGER;
    UPDATE metadata SET schema_version = 4;
"#;

enum UpgradeAction {
    UpToDate,
    DoUpdate {
        script: &'static str,
        new_version: SchemaVersion,
    },
    Outdated,
    Future,
}

enum DatabaseType {
    /// If the Option is none, the database is from an older version of Anchor where we did not
    /// track the schema version yet. We can change the type to "SchemaVersion" at some point and
    /// treat older versions as "Unknown".
    Anchor(Option<SchemaVersion>),
    /// Database belongs to a different network
    IncorrectNetwork(String),
    Unknown,
}

/// Ensure that there is an up-to-date database available at `db_path`. Also check or set the
/// network name to ensure the database is for the correct network.
pub fn ensure_up_to_date(
    db_path: impl AsRef<Path>,
    network_name: &str,
) -> Result<(), DatabaseError> {
    let db_path = db_path.as_ref();
    let is_new_file = !db_path.exists();
    let conn = Connection::open(db_path)?;

    let mut schema_version = if is_new_file {
        Some(create_initial_schema(&conn, network_name)?)
    } else {
        match determine_database_type(&conn, network_name) {
            DatabaseType::Anchor(schema_version) => schema_version,
            DatabaseType::Unknown => {
                // We do not know what this is. Let's be safe and error out.
                return Err(DatabaseError::AlreadyPresent(
                    "Unknown database schema".to_string(),
                ));
            }
            DatabaseType::IncorrectNetwork(stored_network) => {
                return Err(DatabaseError::AlreadyPresent(format!(
                    "Database is for network '{stored_network}', expected '{network_name}'"
                )));
            }
        }
    };

    // Upgrade scripts are step by step, so we need to loop until we are up to date.
    loop {
        match get_upgrade_action(schema_version) {
            UpgradeAction::UpToDate => {
                // After all upgrades, ensure network_name is set (for migrated databases)
                conn.execute(
                    "UPDATE metadata SET network_name = ?1 WHERE network_name IS NULL",
                    [network_name],
                )?;
                return Ok(());
            }
            UpgradeAction::DoUpdate {
                script,
                new_version,
            } => {
                conn.execute_batch(script)?;
                schema_version = Some(new_version);
            }
            UpgradeAction::Outdated => {
                return Err(DatabaseError::AlreadyPresent(
                    "Database is outdated - please remove \"anchor_db.sqlite\" or use another data dir.".to_string(),
                ));
            }
            UpgradeAction::Future => {
                return Err(DatabaseError::AlreadyPresent(
                    "Database schema is newer than supported by this version of Anchor".to_string(),
                ));
            }
        }
    }
}

fn determine_database_type(conn: &Connection, network_name: &str) -> DatabaseType {
    // First, try to get the schema version from metadata table
    let schema_version_result: Result<SchemaVersion, _> =
        conn.query_row(sql_operations::GET_METADATA, [], |row| {
            row.get("schema_version")
        });

    match schema_version_result {
        Ok(schema_version) => {
            // Metadata table exists. Now try to get network_name (may not exist in v1/v2)
            let network_result: Result<Option<String>, _> =
                conn.query_row("SELECT network_name FROM metadata", [], |row| row.get(0));

            match network_result {
                Ok(Some(stored)) if stored == network_name => {
                    DatabaseType::Anchor(Some(schema_version))
                }
                Ok(Some(stored)) => DatabaseType::IncorrectNetwork(stored),
                Ok(None) | Err(_) => {
                    // Either network_name is NULL or the column doesn't exist (v1/v2 database)
                    // Accept for migration
                    DatabaseType::Anchor(Some(schema_version))
                }
            }
        }
        Err(_) => {
            // Metadata table doesn't exist or query failed.
            // Check if this is a legacy Anchor database (pre-metadata table).
            let legacy = conn
                .query_row(sql_operations::GET_LEGACY_BLOCK, [], |row| {
                    // Check if there is the expected column and no further columns.
                    Ok(
                        row.get::<_, u64>("block_number").is_ok()
                            && row.get::<_, Value>(1).is_err(),
                    )
                })
                .unwrap_or(false);

            if legacy {
                DatabaseType::Anchor(None)
            } else {
                DatabaseType::Unknown
            }
        }
    }
}

// Before release, update the return value of this function if the initial table schema was changed.
pub(crate) fn create_initial_schema(
    conn: &rusqlite::Connection,
    network_name: &str,
) -> Result<SchemaVersion, DatabaseError> {
    conn.execute_batch(include_str!("table_schema.sql"))?;
    conn.execute(sql_operations::INSERT_METADATA, [network_name])?;
    let schema_version = conn.query_row(sql_operations::GET_METADATA, [], |row| {
        row.get("schema_version")
    })?;
    Ok(schema_version)
}

// Register upgrade scripts in this function and mark the current version. Define any versions for
// which the schema is not upgradable as "Outdated" and all versions after the current version as
// "Future".
fn get_upgrade_action(version: Option<SchemaVersion>) -> UpgradeAction {
    match version {
        None | Some(0) => UpgradeAction::Outdated,
        Some(1) => UpgradeAction::DoUpdate {
            script: MIGRATION_V1_TO_V2,
            new_version: 2,
        },
        Some(2) => UpgradeAction::DoUpdate {
            script: MIGRATION_V2_TO_V3,
            new_version: 3,
        },
        Some(3) => UpgradeAction::DoUpdate {
            script: MIGRATION_V3_TO_V4,
            new_version: 4,
        },
        Some(4) => UpgradeAction::UpToDate,
        Some(5..) => UpgradeAction::Future,
    }
}
