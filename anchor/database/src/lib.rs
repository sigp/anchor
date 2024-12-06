use r2d2_sqlite::SqliteConnectionManager;
use ssv_types::{Cluster, ClusterId};
use ssv_types::{Operator, OperatorId, Share};
use std::collections::HashMap;
use std::fs::File;
use std::path::Path;
use std::time::Duration;
use types::PublicKey;

mod cluster_operations;
pub mod error;
mod operator_operations;
mod share_operations;
mod validator_operations;

pub use crate::error::DatabaseError;

#[cfg(test)]
pub mod test_utils;

// Todo
// 1) Decide on the types I want to use
// 2) Rebuilding after restart
// 3) Validator logic
// 4) To/From sql for all the types
// 5) Test

type Pool = r2d2::Pool<SqliteConnectionManager>;

pub const POOL_SIZE: u32 = 1;
pub const CONNECTION_TIMEOUT: Duration = Duration::from_secs(5);

/// Top level NetworkDatabase that contains in memory storage to relevant information for quick
/// access and a connection to the underlying database
#[derive(Debug, Clone)]
pub struct NetworkDatabase {
    /// All of the operators in the network
    operators: HashMap<OperatorId, Operator>,
    /// The clusters that this operator is a member in
    clusters: HashMap<ClusterId, Cluster>,
    /// The shares that this operator is responsible for
    shares: HashMap<PublicKey, Share>,
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

        let db = Self {
            operators: HashMap::new(),
            clusters: HashMap::new(),
            shares: HashMap::new(),
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

        // populate stores
        // todo!()

        Ok(Self {
            operators: HashMap::new(),
            clusters: HashMap::new(),
            shares: HashMap::new(),
            conn_pool,
        })
    }

    // Open a new connection
    fn connection(&self) -> Result<r2d2::PooledConnection<SqliteConnectionManager>, DatabaseError> {
        Ok(self.conn_pool.get()?)
    }

    // Populate in memory share store with the shares that this operator owns
    fn populate_shares(_conn: &Pool) -> HashMap<PublicKey, Share> {
        todo!()
    }

    // Populate the in memory operator store with all of the operators in the network
    fn populate_operators(_conn: &Pool) -> HashMap<OperatorId, Operator> {
        todo!()
    }

    // Populate the in memory cluster store with all of the clusters that this operator is a
    // member of
    fn populate_clusters(_conn: &Pool) -> HashMap<ClusterId, Cluster> {
        todo!()
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
}

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
