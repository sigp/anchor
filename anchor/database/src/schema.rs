use std::path::Path;

use refinery::{Target, embed_migrations};
use rusqlite::{Connection, OptionalExtension, params, types::Value};

use crate::{DatabaseError, sql_operations};

embed_migrations!("src/migrations");

type SchemaVersion = u32;

const SUPPORTED_PRE_REFINERY_SCHEMA_VERSION: SchemaVersion = 1;
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

#[cfg(test)]
pub(crate) fn stamp_baseline_for_tests(conn: &mut Connection) -> Result<(), DatabaseError> {
    migration_runner()
        .set_target(Target::FakeVersion(BASELINE_MIGRATION_VERSION))
        .run(conn)?;
    Ok(())
}

#[cfg(test)]
pub(crate) fn run_migrations_for_tests(conn: &mut Connection) -> Result<(), DatabaseError> {
    migration_runner().run(conn)?;
    Ok(())
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
            bridge_manual_anchor_database(conn, schema_version)?;
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
        // Once refinery owns the DB, `refinery_schema_history` is the authoritative signal. The
        // legacy `metadata.schema_version` column may still exist for compatibility, but it is no
        // longer the source of truth for schema state. A DB can also be in the stamped-only V1
        // bridge state after `FakeVersion(1)` but before V2 has added `network_name`, so guard
        // the read the same way we do for manual schema-v1 DBs.
        let stored_network = if has_metadata && has_column(conn, "metadata", "network_name")? {
            conn.query_row("SELECT network_name FROM metadata", [], |row| row.get(0))
                .optional()?
                .flatten()
        } else {
            None
        };

        return Ok(DatabaseType::RefineryManaged { stored_network });
    }

    if has_metadata {
        // A metadata table without refinery history is the pre-cutover manual Anchor path. Those
        // DBs are classified by the legacy `schema_version` field so we can decide whether to
        // adopt them into the refinery baseline or reject them as unsupported.
        let schema_version = conn
            .query_row("SELECT schema_version FROM metadata", [], |row| row.get(0))
            .optional()?;

        if let Some(schema_version) = schema_version {
            // Shipped schema-v1 databases do not have `network_name` yet, so check for the column
            // before reading it. Later manual schemas would have it, and we still want to enforce
            // cross-network safety before doing any bridge work.
            let stored_network = if has_column(conn, "metadata", "network_name")? {
                conn.query_row("SELECT network_name FROM metadata", [], |row| row.get(0))
                    .optional()?
                    .flatten()
            } else {
                None
            };

            return Ok(DatabaseType::ManualAnchor {
                schema_version,
                stored_network,
            });
        }

        // A metadata table with no singleton row is not a valid refinery-era or supported manual
        // Anchor state. Treat it like an unsupported legacy DB rather than trying to infer shape
        // from half-initialized contents.
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
) -> Result<(), DatabaseError> {
    if schema_version != SUPPORTED_PRE_REFINERY_SCHEMA_VERSION {
        return Err(DatabaseError::AlreadyPresent(
            "Database is from an unsupported pre-refinery Anchor version. Please remove \"anchor_db.sqlite\" and let Anchor recreate it."
                .to_string(),
        ));
    }

    // Manual schema v1 is now the refinery baseline, so pre-refinery production databases can be
    // adopted by stamping V1 as already applied and then letting refinery run the combined V2
    // upgrade normally. This avoids replaying synthetic historical states: the DB stays in place,
    // refinery history is bootstrapped once, and all later evolution goes through real migration
    // files. Stamping V1 and then applying V2 happen in separate runner invocations, so the
    // bridge+upgrade path is resumable rather than fully atomic: if V1 is stamped and V2 fails,
    // the next startup will see a refinery-managed DB and retry V2.
    migration_runner()
        .set_target(Target::FakeVersion(BASELINE_MIGRATION_VERSION))
        .run(conn)?;

    Ok(())
}

fn ensure_metadata_row(conn: &Connection, network_name: &str) -> Result<(), DatabaseError> {
    conn.execute(sql_operations::INSERT_METADATA, params![network_name])?;
    // V2 mirrors the old additive path, so pre-refinery rows still need runtime normalization
    // after the migration runs. Fresh DBs already inserted the full row through INSERT_METADATA;
    // adopted schema-v1 DBs still need the new columns populated and the legacy columns brought to
    // the post-upgrade values expected by the rest of the code.
    conn.execute(
        "UPDATE metadata
         SET schema_version = 4,
             domain_type = COALESCE(domain_type, 0),
             network_name = COALESCE(network_name, ?1)",
        params![network_name],
    )?;
    Ok(())
}

fn has_table(conn: &Connection, table_name: &str) -> Result<bool, DatabaseError> {
    // Use sqlite_master as a pure existence check. `.optional()` keeps the common "not present"
    // case cheap while still propagating real SQL errors instead of misclassifying broken DBs.
    conn.query_row(
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1 LIMIT 1",
        [table_name],
        |_| Ok(()),
    )
    .optional()
    .map(|exists| exists.is_some())
    .map_err(DatabaseError::from)
}

fn has_column(
    conn: &Connection,
    table_name: &str,
    column_name: &str,
) -> Result<bool, DatabaseError> {
    // Legacy manual schema-v1 databases legitimately lack newer columns such as `network_name`, so
    // callers need an existence probe that preserves real SQL failures.
    conn.query_row(
        "SELECT 1 FROM pragma_table_info(?1) WHERE name = ?2 LIMIT 1",
        params![table_name, column_name],
        |_| Ok(()),
    )
    .optional()
    .map(|exists| exists.is_some())
    .map_err(DatabaseError::from)
}

fn is_empty_database(conn: &Connection) -> Result<bool, DatabaseError> {
    let objects: u64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE name NOT LIKE 'sqlite_%'",
        [],
        |row| row.get(0),
    )?;
    Ok(objects == 0)
}
