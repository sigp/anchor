use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::params;
use ssv_types::{Cluster, ClusterId};
use ssv_types::{Operator, OperatorId, Share};
use std::collections::HashMap;
use std::fs::File;
use std::path::Path;
use std::time::Duration;
use types::PublicKey;

mod cluster_operations;
mod operator_operations;
mod share_operations;
mod validator_operations;

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

#[derive(Debug, Clone)]
pub struct NetworkDatabase {
    operators: HashMap<OperatorId, Operator>,
    clusters: HashMap<ClusterId, Cluster>,
    shares: HashMap<PublicKey, Share>,
    conn_pool: Pool,
}

impl NetworkDatabase {
    /// Open an existing database at the given `path`, or create one if none exists.
    pub fn open_or_create(path: &Path) -> Result<Self, String> {
        if path.exists() {
            Self::open(path)
        } else {
            Self::create(path)
        }
    }

    fn connection(&self) -> Result<r2d2::PooledConnection<SqliteConnectionManager>, String> {
        self.conn_pool
            .get()
            .map_err(|e| format!("Unable to get db connection: {:?}", e))
    }

    /// Create a `NetworkDatabase` at the given path.
    pub fn create(path: &Path) -> Result<Self, String> {
        let _file = File::options()
            .write(true)
            .read(true)
            .create_new(true)
            .open(path)
            .map_err(|e| format!("Unable to create file at path {:?}: {}", path, e))?;

        // restrict file permissions
        let conn_pool = Self::open_conn_pool(path)?;
        let conn = conn_pool
            .get()
            .map_err(|e| format!("Unable to get connection to the database: {:?}", e))?;

        // Operator table
        conn.execute(
            "CREATE TABLE operators (
                    operator_id INTEGER PRIMARY KEY,
                    public_key TEXT NOT NULL,
                    owner_address TEXT NOT NULL,
                    UNIQUE (public_key)
            )",
            params![],
        )
        .map_err(|e| format!("Unable to create operators table in database: {:?}", e))?;

        // Create clusters table - another parent table with no dependencies
        conn.execute(
            "CREATE TABLE clusters (
                cluster_id INTEGER PRIMARY KEY,
                faulty INTEGER NOT NULL,
                liquidated BOOLEAN DEFAULT FALSE
            )",
            params![],
        )
        .map_err(|e| format!("Unable to create clusters table: {:?}", e))?;

        // Create cluster_members table - depends on both operators and clusters
        conn.execute(
            "CREATE TABLE cluster_members (
                cluster_id INTEGER NOT NULL,
                operator_id INTEGER NOT NULL,
                PRIMARY KEY (cluster_id, operator_id),
                FOREIGN KEY (cluster_id) REFERENCES clusters(cluster_id) ON DELETE CASCADE,
                FOREIGN KEY (operator_id) REFERENCES operators(operator_id) ON DELETE CASCADE
            )",
            params![],
        )
        .map_err(|e| format!("Unable to create cluster_members table: {:?}", e))?;

        // Create validators table - depends on clusters
        conn.execute(
            "CREATE TABLE validators (
                validator_pubkey TEXT PRIMARY KEY,
                cluster_id INTEGER NOT NULL,
                fee_recipient TEXT,
                graffiti BLOB,
                validator_index INTEGER,
                last_updated TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                FOREIGN KEY (cluster_id) REFERENCES clusters(cluster_id) ON DELETE CASCADE
            )",
            params![],
        )
        .map_err(|e| format!("Unable to create validators table: {:?}", e))?;

        // Create shares table - depends on validators and cluster_members
        conn.execute(
        "CREATE TABLE shares (
                validator_pubkey TEXT NOT NULL,
                cluster_id INTEGER NOT NULL,
                operator_id INTEGER NOT NULL,
                share_pubkey TEXT,
                PRIMARY KEY (validator_pubkey, operator_id),
                FOREIGN KEY (cluster_id, operator_id) REFERENCES cluster_members(cluster_id, operator_id) ON DELETE CASCADE,
                FOREIGN KEY (validator_pubkey) REFERENCES validators(validator_pubkey) ON DELETE CASCADE
            )",
            params![],
        ).map_err(|e| format!("Unable to create shares table: {:?}", e))?;

        Ok(Self {
            operators: HashMap::new(),
            clusters: HashMap::new(),
            shares: HashMap::new(),
            conn_pool,
        })
    }

    /// Open an existing `NetworkDatabase` from disk.
    pub fn open(path: &Path) -> Result<Self, String> {
        let conn_pool = Self::open_conn_pool(path)?;

        // Populate all in memory data w/ db connection
        let operators = Self::populate_operators(&conn_pool);
        let shares = Self::populate_shares(&conn_pool, &operators);

        let db = Self {
            operators,
            clusters: HashMap::new(),
            shares: HashMap::new(),
            conn_pool,
        };
        Ok(db)
    }

    // populate in memory share store
    fn populate_shares(
        _conn: &Pool,
        _operators: &HashMap<OperatorId, Operator>,
    ) -> HashMap<PublicKey, Share> {
        todo!()
    }

    // populate in memory operator store w/ existing database entries
    fn populate_operators(_conn: &Pool) -> HashMap<OperatorId, Operator> {
        todo!()
    }

    fn open_conn_pool(path: &Path) -> Result<Pool, String> {
        let manager = SqliteConnectionManager::file(path);
        // some other args here
        let conn_pool = Pool::builder()
            .max_size(POOL_SIZE)
            .connection_timeout(CONNECTION_TIMEOUT)
            .build(manager)
            .map_err(|e| format!("Unable to open database: {:?}", e))?;
        Ok(conn_pool)
    }
}

#[cfg(test)]
mod database_test {
    use super::*;

    #[test]
    fn test_create_database() {
        let path = Path::new("db");
        let db = NetworkDatabase::open_or_create(path);
        assert!(db.is_ok());
    }
}
