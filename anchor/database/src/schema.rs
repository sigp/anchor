use std::path::Path;

use rusqlite::{Connection, types::Value};
use ssv_types::domain_type::DomainType;

use crate::{DatabaseError, sql_operations};

type SchemaVersion = u32;

struct Metadata {
    schema_version: SchemaVersion,
    domain: DomainType,
}

/// Migration from schema version 1 to 2: Add max_operator_id_seen column to metadata table
const MIGRATION_V1_TO_V2: &str = r#"
    ALTER TABLE metadata ADD COLUMN max_operator_id_seen INTEGER;
    UPDATE metadata SET schema_version = 2;
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
    IncorrectDomain(DomainType),
    Unknown,
}

/// Ensure that there is an up-to-date database available at `db_path`. Also check or set the
/// domain type to ensure the database is for the correct network.
pub fn ensure_up_to_date(
    db_path: impl AsRef<Path>,
    domain: DomainType,
) -> Result<(), DatabaseError> {
    let db_path = db_path.as_ref();
    let is_new_file = !db_path.exists();
    let conn = Connection::open(db_path)?;

    let mut schema_version = if is_new_file {
        Some(create_initial_schema(&conn, domain)?)
    } else {
        match determine_database_type(&conn, domain) {
            DatabaseType::Anchor(schema_version) => schema_version,
            DatabaseType::Unknown => {
                // We do not know what this is. Let's be safe and error out.
                return Err(DatabaseError::AlreadyPresent(
                    "Unknown database schema".to_string(),
                ));
            }
            DatabaseType::IncorrectDomain(domain) => {
                return Err(DatabaseError::AlreadyPresent(format!(
                    "Existing database for different network: {domain:?}"
                )));
            }
        }
    };

    // Upgrade scripts are step by step, so we need to loop until we are up to date.
    loop {
        match get_upgrade_action(schema_version) {
            UpgradeAction::UpToDate => return Ok(()),
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

fn determine_database_type(conn: &Connection, domain: DomainType) -> DatabaseType {
    let result = conn.query_row(sql_operations::GET_METADATA, [], |row| {
        Ok(Metadata {
            schema_version: row.get("schema_version")?,
            domain: row.get("domain_type")?,
        })
    });

    match result {
        Ok(metadata) => {
            if metadata.domain == domain {
                DatabaseType::Anchor(Some(metadata.schema_version))
            } else {
                DatabaseType::IncorrectDomain(metadata.domain)
            }
        }
        Err(_) => {
            // Something failed - this might be a non-Anchor or legacy Anchor database.
            // To check, try to get the block from the old table before `metadata` was introduced.
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
    domain: DomainType,
) -> Result<SchemaVersion, DatabaseError> {
    conn.execute_batch(include_str!("table_schema.sql"))?;
    conn.execute(sql_operations::INSERT_METADATA, [&domain])?;
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
        Some(2) => UpgradeAction::UpToDate,
        Some(3..) => UpgradeAction::Future,
    }
}
