use std::path::Path;

use refinery::{Target, embed_migrations};
use rusqlite::{Connection, OptionalExtension, params, types::Value};

use crate::{DatabaseError, sql_operations};

embed_migrations!("src/migrations");

type SchemaVersion = u32;

const LATEST_SCHEMA_VERSION: SchemaVersion = 4;
const SUPPORTED_PRE_REFINERY_SCHEMA_VERSION: SchemaVersion = 3;
const BASELINE_MIGRATION_VERSION: i32 = 1;

enum DatabaseType {
    New,
    RefineryManaged {
        stored_network: Option<String>,
    },
    ManualAnchor {
        schema_version: SchemaVersion,
        stored_network: Option<String>,
    },
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
        DatabaseType::RefineryManaged { stored_network } => {
            validate_network_name(stored_network.as_deref(), network_name)?;
        }
        DatabaseType::ManualAnchor {
            schema_version,
            stored_network,
        } => {
            validate_network_name(stored_network.as_deref(), network_name)?;
            bridge_manual_anchor_database(conn, schema_version, network_name)?;
        }
        DatabaseType::LegacyUnsupported => {
            return Err(DatabaseError::AlreadyPresent(
                "Database is from an unsupported pre-refinery Anchor version. Please remove \"anchor_db.sqlite\" and let Anchor recreate it."
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

    if has_refinery_history {
        let stored_network = if has_metadata {
            conn.query_row("SELECT network_name FROM metadata", [], |row| row.get(0))
                .optional()?
                .flatten()
        } else {
            None
        };

        return Ok(DatabaseType::RefineryManaged { stored_network });
    }

    if has_metadata {
        let schema_version = conn
            .query_row("SELECT schema_version FROM metadata", [], |row| row.get(0))
            .optional()?;

        if let Some(schema_version) = schema_version {
            let stored_network = conn
                .query_row("SELECT network_name FROM metadata", [], |row| row.get(0))
                .ok()
                .flatten();

            return Ok(DatabaseType::ManualAnchor {
                schema_version,
                stored_network,
            });
        }

        return Ok(DatabaseType::LegacyUnsupported);
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

fn bridge_manual_anchor_database(
    conn: &mut Connection,
    schema_version: SchemaVersion,
    network_name: &str,
) -> Result<(), DatabaseError> {
    if schema_version != SUPPORTED_PRE_REFINERY_SCHEMA_VERSION {
        return Err(DatabaseError::AlreadyPresent(
            "Database is from an unsupported pre-refinery Anchor version. Please remove \"anchor_db.sqlite\" and let Anchor recreate it."
                .to_string(),
        ));
    }

    canonicalize_manual_v3_metadata(conn, network_name)?;
    migration_runner()
        .set_target(Target::FakeVersion(BASELINE_MIGRATION_VERSION))
        .run(conn)?;

    Ok(())
}

fn canonicalize_manual_v3_metadata(
    conn: &Connection,
    network_name: &str,
) -> Result<(), DatabaseError> {
    // Some shipped schema-v3 databases were created from the full v3 schema, while others reached
    // v3 through additive ALTER TABLE migrations and therefore have a weaker metadata definition.
    // Normalize both shapes to the canonical production baseline before stamping V1 as applied.
    conn.execute_batch(
        "DROP TRIGGER IF EXISTS unique_metadata;
         CREATE TABLE metadata_new (
             schema_version INTEGER NOT NULL DEFAULT 3,
             domain_type INTEGER NOT NULL DEFAULT 0,
             network_name TEXT NOT NULL,
             block_number INTEGER NOT NULL DEFAULT 0 CHECK (block_number >= 0),
             max_operator_id_seen INTEGER DEFAULT 0
         );",
    )?;

    conn.execute(
        "INSERT INTO metadata_new (
             schema_version,
             domain_type,
             network_name,
             block_number,
             max_operator_id_seen
         )
         SELECT
             ?1,
             COALESCE(domain_type, 0),
             COALESCE(network_name, ?2),
             block_number,
             COALESCE(max_operator_id_seen, 0)
         FROM metadata",
        params![SUPPORTED_PRE_REFINERY_SCHEMA_VERSION, network_name],
    )?;

    conn.execute_batch(
        "DROP TABLE metadata;
         ALTER TABLE metadata_new RENAME TO metadata;
         CREATE TRIGGER unique_metadata
             BEFORE INSERT ON metadata
             WHEN (SELECT COUNT(*) FROM metadata) >= 1
         BEGIN
             SELECT RAISE(FAIL, 'we can only have one metadata row');
         END;",
    )?;

    Ok(())
}

fn ensure_metadata_row(conn: &Connection, network_name: &str) -> Result<(), DatabaseError> {
    conn.execute(
        sql_operations::INSERT_METADATA,
        params![LATEST_SCHEMA_VERSION, network_name],
    )?;
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
