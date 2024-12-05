use r2d2_sqlite::SqliteConnectionManager;
use rsa::RsaPublicKey;
use rusqlite::params;
use ssv_types::{Operator, OperatorId, Share};
use std::collections::HashMap;
use std::fs::File;
use std::path::Path;
use std::time::Duration;
use types::{Address, PublicKey};

mod operator_operations;

type Pool = r2d2::Pool<SqliteConnectionManager>;

pub const POOL_SIZE: u32 = 1;
pub const CONNECTION_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone)]
pub struct NetworkDatabase {
    /// OperatorID => Operator
    operators: HashMap<OperatorId, Operator>,
    /// ValidatorPublickKey => Share
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

        Ok(Self {
            operators: HashMap::new(),
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
            shares,
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
