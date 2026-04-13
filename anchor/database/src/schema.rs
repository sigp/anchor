use std::path::Path;

use refinery::{Target, embed_migrations};
use rusqlite::{Connection, OptionalExtension, params, types::Value};

use crate::{DatabaseError, sql_operations};

embed_migrations!("src/migrations");

type SchemaVersion = u32;

const LATEST_SCHEMA_VERSION: SchemaVersion = 4;
const MIN_SUPPORTED_SCHEMA_VERSION: SchemaVersion = 1;

#[derive(Debug)]
struct AnchorDatabaseState {
    schema_version: SchemaVersion,
    stored_network: Option<String>,
    has_refinery_history: bool,
}

enum DatabaseType {
    New,
    Anchor(AnchorDatabaseState),
    LegacyUnsupported,
    Unknown,
}

fn migration_runner() -> refinery::Runner {
    // Treat the pending startup migrations as one unit so we do not leave the database halfway
    // upgraded if a later step fails.
    migrations::runner().set_grouped(true)
}

/// Ensure that there is an up-to-date database available at `db_path`. Also check or set the
/// network name to ensure the database is for the correct network.
pub fn ensure_up_to_date(
    db_path: impl AsRef<Path>,
    network_name: &str,
) -> Result<(), DatabaseError> {
    let db_path = db_path.as_ref();
    let is_new_file = !db_path.exists();
    let mut conn = Connection::open(db_path)?;
    ensure_up_to_date_with_connection(&mut conn, network_name, is_new_file)
}

#[cfg(feature = "test-utils")]
pub(crate) fn initialize_in_memory(
    conn: &mut Connection,
    network_name: &str,
) -> Result<(), DatabaseError> {
    ensure_up_to_date_with_connection(conn, network_name, true)
}

#[cfg(test)]
pub(crate) fn migration_runner_for_tests() -> refinery::Runner {
    migration_runner()
}

fn ensure_up_to_date_with_connection(
    conn: &mut Connection,
    network_name: &str,
    assume_new_database: bool,
) -> Result<(), DatabaseError> {
    let database_type = if assume_new_database && is_empty_database(conn)? {
        DatabaseType::New
    } else {
        determine_database_type(conn)?
    };

    match database_type {
        DatabaseType::New => {}
        DatabaseType::Anchor(state) => {
            validate_network_name(state.stored_network.as_deref(), network_name)?;
            if !state.has_refinery_history {
                // Existing Anchor databases predate `refinery_schema_history`. Seed the history
                // from the recorded schema version so we can adopt the DB in place.
                bootstrap_refinery_history(conn, state.schema_version)?;
            }
        }
        DatabaseType::LegacyUnsupported => {
            return Err(DatabaseError::AlreadyPresent(
                "Database is outdated - please remove \"anchor_db.sqlite\" or use another data dir."
                    .to_string(),
            ));
        }
        DatabaseType::Unknown => {
            return Err(DatabaseError::AlreadyPresent(
                "Unknown database schema".to_string(),
            ));
        }
    }

    migration_runner().run(conn)?;
    ensure_metadata_row(conn, network_name)?;
    Ok(())
}

fn determine_database_type(conn: &Connection) -> Result<DatabaseType, DatabaseError> {
    let has_metadata = has_table(conn, "metadata")?;
    let has_refinery_history = has_table(conn, "refinery_schema_history")?;

    if has_metadata {
        let schema_version = conn
            .query_row("SELECT schema_version FROM metadata", [], |row| row.get(0))
            .optional()?;

        if let Some(schema_version) = schema_version {
            let stored_network = conn
                .query_row("SELECT network_name FROM metadata", [], |row| row.get(0))
                .ok();

            return Ok(DatabaseType::Anchor(AnchorDatabaseState {
                schema_version,
                stored_network,
                has_refinery_history,
            }));
        }

        if has_refinery_history {
            return Ok(DatabaseType::Anchor(AnchorDatabaseState {
                schema_version: LATEST_SCHEMA_VERSION,
                stored_network: None,
                has_refinery_history,
            }));
        }
    }

    let legacy = conn
        .query_row(sql_operations::GET_LEGACY_BLOCK, [], |row| {
            Ok(row.get::<_, u64>("block_number").is_ok() && row.get::<_, Value>(1).is_err())
        })
        .unwrap_or(false);

    if legacy {
        Ok(DatabaseType::LegacyUnsupported)
    } else if is_empty_database(conn)? {
        Ok(DatabaseType::New)
    } else {
        Ok(DatabaseType::Unknown)
    }
}

fn validate_network_name(
    stored_network: Option<&str>,
    expected_network: &str,
) -> Result<(), DatabaseError> {
    if let Some(stored_network) = stored_network
        && stored_network != expected_network
    {
        return Err(DatabaseError::AlreadyPresent(format!(
            "Database is for network '{stored_network}', expected '{expected_network}'"
        )));
    }

    Ok(())
}

fn bootstrap_refinery_history(
    conn: &mut Connection,
    schema_version: SchemaVersion,
) -> Result<(), DatabaseError> {
    if schema_version < MIN_SUPPORTED_SCHEMA_VERSION {
        return Err(DatabaseError::AlreadyPresent(
            "Database is outdated - please remove \"anchor_db.sqlite\" or use another data dir."
                .to_string(),
        ));
    }

    if schema_version > LATEST_SCHEMA_VERSION {
        return Err(DatabaseError::AlreadyPresent(
            "Database schema is newer than supported by this version of Anchor".to_string(),
        ));
    }

    // The on-disk schema and data already exist. We only need `refinery` to record which
    // migrations should be considered applied before running any newer ones.
    migration_runner()
        .set_target(Target::FakeVersion(schema_version.try_into().map_err(
            |_| {
                DatabaseError::MigrationError(format!(
                    "schema version {schema_version} cannot be converted to usize"
                ))
            },
        )?))
        .run(conn)?;
    Ok(())
}

fn ensure_metadata_row(conn: &Connection, network_name: &str) -> Result<(), DatabaseError> {
    conn.execute(
        sql_operations::INSERT_METADATA,
        params![LATEST_SCHEMA_VERSION, network_name],
    )?;
    // Historical migrations cannot know the correct runtime network for old DBs. Fill it here
    // after the caller has validated which network this process is opening.
    conn.execute(
        "UPDATE metadata SET schema_version = ?1, network_name = COALESCE(network_name, ?2)",
        params![LATEST_SCHEMA_VERSION, network_name],
    )?;
    Ok(())
}

fn has_table(conn: &Connection, table_name: &str) -> Result<bool, DatabaseError> {
    let exists = conn.query_row(
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1 LIMIT 1",
        [table_name],
        |_| Ok(()),
    );
    Ok(exists.is_ok())
}

fn is_empty_database(conn: &Connection) -> Result<bool, DatabaseError> {
    let objects: u64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE name NOT LIKE 'sqlite_%'",
        [],
        |row| row.get(0),
    )?;
    Ok(objects == 0)
}
