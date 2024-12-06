pub use crate::error::DatabaseError;
use r2d2_sqlite::SqliteConnectionManager;
use ssv_types::{ClusterId, ValidatorMetadata};
use ssv_types::{Operator, OperatorId, Share};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::path::Path;
use std::sync::LazyLock;
use std::time::Duration;

mod cluster_operations;
pub mod error;
mod operator_operations;
mod share_operations;
mod validator_operations;

#[cfg(test)]
pub mod test_utils;

type Pool = r2d2::Pool<SqliteConnectionManager>;
pub const POOL_SIZE: u32 = 1;
pub const CONNECTION_TIMEOUT: Duration = Duration::from_secs(5);

/// Top level NetworkDatabase that contains in memory storage for quick access
/// to relevant information and a connection to the database
#[derive(Debug, Clone)]
pub struct NetworkDatabase {
    /// All of the operators in the network
    operators: HashMap<OperatorId, Operator>,
    /// All of the clusters in the networ
    clusters: HashSet<ClusterId>,
    /// Mapping of a cluster ID to its relevant Validator metadata
    validator_metadata: HashMap<ClusterId, ValidatorMetadata>,
    /// Double layer share map from Cluster => Operator => Share
    shares: HashMap<ClusterId, HashMap<OperatorId, Share>>,
    /// Maps a ClusterID to the operators in its cluster
    cluster_members: HashMap<ClusterId, HashSet<OperatorId>>,
    /// Connection to the database
    conn_pool: Pool,
}

impl NetworkDatabase {
    /// Open an existing database at the given `path`, or create one if none exists.
    pub fn open_or_create(path: &Path) -> Result<Self, DatabaseError> {
        if path.exists() {
            Self::open(path)
        } else {
            Self::create(path)
        }
    }

    // Open an existing `NetworkDatabase` from disk.
    fn open(path: &Path) -> Result<Self, DatabaseError> {
        let conn_pool = Self::open_conn_pool(path)?;

        // todo!(): populate in memory stores

        let db = Self {
            operators: HashMap::new(),
            clusters: HashSet::new(),
            validator_metadata: HashMap::new(),
            shares: HashMap::new(),
            cluster_members: HashMap::new(),
            conn_pool,
        };
        Ok(db)
    }

    /// Create a `NetworkDatabase` at the given path.
    pub fn create(path: &Path) -> Result<Self, DatabaseError> {
        let _file = File::options()
            .write(true)
            .read(true)
            .create_new(true)
            .open(path)?;

        // restrict file permissions
        let conn_pool = Self::open_conn_pool(path)?;
        let conn = conn_pool.get()?;

        // create all of the tables
        conn.execute_batch(include_str!("table_schema.sql"))?;

        // todo!() populate in memory stores

        Ok(Self {
            operators: HashMap::new(),
            clusters: HashSet::new(),
            validator_metadata: HashMap::new(),
            shares: HashMap::new(),
            cluster_members: HashMap::new(),
            conn_pool,
        })
    }

    /// Build a new connection pool
    fn open_conn_pool(path: &Path) -> Result<Pool, DatabaseError> {
        let manager = SqliteConnectionManager::file(path);
        // some other args here
        let conn_pool = Pool::builder()
            .max_size(POOL_SIZE)
            .connection_timeout(CONNECTION_TIMEOUT)
            .build(manager)?;
        Ok(conn_pool)
    }

    // Open a new connection
    fn connection(&self) -> Result<r2d2::PooledConnection<SqliteConnectionManager>, DatabaseError> {
        Ok(self.conn_pool.get()?)
    }
}

// Wrappers around various SQL statements used for interacting with the db
#[derive(Debug, Hash, Eq, PartialEq, Clone, Copy)]
pub(crate) enum SqlStatement {
    InsertOperator,
    DeleteOperator,

    InsertCluster,
    InsertClusterMember,
    UpdateClusterStatus,
    UpdateClusterFaulty,
    DeleteCluster,

    InsertShare,
    InsertValidator,
    UpdateFeeRecipient,
    SetGraffiti,
    SetValidatorIndex,
}

pub(crate) static SQL: LazyLock<HashMap<SqlStatement, &'static str>> = LazyLock::new(|| {
    let mut m = HashMap::new();
    m.insert(
        SqlStatement::InsertOperator,
        "INSERT INTO operators (operator_id, public_key, owner_address) VALUES (?1, ?2, ?3)",
    );
    m.insert(
        SqlStatement::DeleteOperator,
        "DELETE FROM operators WHERE operator_id = ?1",
    );
    m.insert(
        SqlStatement::InsertCluster,
        "INSERT INTO clusters (cluster_id) VALUES (?1)",
    );
    m.insert(
        SqlStatement::UpdateClusterStatus,
        "UPDATE clusters SET liquidated = ?1 WHERE cluster_id = ?2",
    );
    m.insert(
        SqlStatement::UpdateClusterFaulty,
        "UPDATE clusters SET faulty = ?1 WHERE cluster_id = ?2",
    );
    m.insert(
        SqlStatement::InsertClusterMember,
        "INSERT INTO cluster_members (cluster_id, operator_id) VALUES (?1, ?2)",
    );
    m.insert(
        SqlStatement::DeleteCluster,
        "DELETE FROM clusters WHERE cluster_id = ?1",
    );
    m.insert(SqlStatement::InsertShare,
        "INSERT INTO shares (validator_pubkey, cluster_id, operator_id, share_pubkey) VALUES (?1, ?2, ?3, ?4)");
    m.insert(
        SqlStatement::InsertValidator,
        "INSERT INTO validators (validator_pubkey, cluster_id) VALUES (?1, ?2)",
    );
    m.insert(
        SqlStatement::UpdateFeeRecipient,
        "UPDATE validators SET fee_recipient = ?1 WHERE validator_pubkey = ?2",
    );
    m.insert(
        SqlStatement::SetGraffiti,
        "UPDATE validators SET graffiti = ?1 WHERE validator_pubkey = ?2",
    );
    m.insert(
        SqlStatement::SetValidatorIndex,
        "UPDATE validators SET validator_index = ?1 WHERE validator_pubkey = ?2",
    );
    m
});

#[cfg(test)]
mod database_test {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_create_database() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("db.sqlite");
        let db = NetworkDatabase::open_or_create(&file);
        assert!(db.is_ok());
    }
}
