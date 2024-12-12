pub use crate::error::DatabaseError;
use openssl::{pkey::Public, rsa::Rsa};
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::params;
use ssv_types::{ClusterId, Operator, OperatorId, Share, ValidatorMetadata};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::path::Path;
use std::sync::LazyLock;
use std::time::Duration;

mod cluster_operations;
mod error;
mod operator_operations;
mod share_operations;
mod state;
mod validator_operations;

#[cfg(test)]
mod tests;

type Pool = r2d2::Pool<SqliteConnectionManager>;
type PoolConn = r2d2::PooledConnection<SqliteConnectionManager>;
const POOL_SIZE: u32 = 1;
const CONNECTION_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Default)]
struct NetworkState {
    /// The ID of our own operator. This is determined via events when the operator is
    /// registered with the network. Therefore, this may not be available right away if the client
    /// is running but has not bee registered with the network contract yet.
    id: Option<OperatorId>,
    /// All of the operators in the network
    operators: HashMap<OperatorId, Operator>,
    /// All of the Clusters that we are a memeber of
    clusters: HashSet<ClusterId>,
    /// All of the shares that we are responsible for/own
    shares: HashMap<ClusterId, Share>,
    /// ValidatorMetadata for clusters we are a member in
    validator_metadata: HashMap<ClusterId, ValidatorMetadata>,
    /// Full set of members for a cluster we are in
    cluster_members: HashMap<ClusterId, HashSet<OperatorId>>,
}

/// Top level NetworkDatabase that contains in memory storage for quick access
/// to relevant information and a connection to the database
#[derive(Debug, Clone)]
pub struct NetworkDatabase {
    /// The public key of our operator
    pubkey: Rsa<Public>,
    /// Custom state stores for easy data access
    state: NetworkState,
    /// Connection to the database
    conn_pool: Pool,
}

impl NetworkDatabase {
    /// Construct a new NetworkDatabase at the given path and the Public Key of our operator.
    pub fn new(path: &Path, pubkey: &Rsa<Public>) -> Result<Self, DatabaseError> {
        let conn_pool = Self::open_or_create(path)?;
        let state = NetworkState::new_with_state(&conn_pool, pubkey)?;
        Ok(Self {
            pubkey: pubkey.clone(),
            state,
            conn_pool,
        })
    }

    /// Update the last processed block number in the database
    pub fn processed_block(&mut self, number: u64) -> Result<(), DatabaseError> {
        let conn = self.connection()?;
        conn.prepare_cached(SQL[&SqlStatement::UpdateBlockNumber])?
            .execute(params![number])?;
        Ok(())
    }

    // Open an existing database at the given `path`, or create one if none exists.
    fn open_or_create(path: &Path) -> Result<Pool, DatabaseError> {
        if path.exists() {
            Self::open_conn_pool(path)
        } else {
            Self::create(path)
        }
    }

    // Build a new connection pool
    fn open_conn_pool(path: &Path) -> Result<Pool, DatabaseError> {
        let manager = SqliteConnectionManager::file(path);
        // some other args here
        let conn_pool = Pool::builder()
            .max_size(POOL_SIZE)
            .connection_timeout(CONNECTION_TIMEOUT)
            .build(manager)?;
        Ok(conn_pool)
    }

    // Create a database at the given path.
    fn create(path: &Path) -> Result<Pool, DatabaseError> {
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
        Ok(conn_pool)
    }

    // Open a new connection
    fn connection(&self) -> Result<PoolConn, DatabaseError> {
        Ok(self.conn_pool.get()?)
    }
}

// Wrappers around various SQL statements used for interacting with the db
#[derive(Debug, Hash, Eq, PartialEq, Clone, Copy)]
pub(crate) enum SqlStatement {
    InsertOperator,
    DeleteOperator,
    GetOperatorId,
    GetAllOperators,

    InsertCluster,
    InsertClusterMember,
    UpdateClusterStatus,
    UpdateClusterFaulty,
    DeleteCluster,
    GetAllClusters,
    GetClusterMembers,

    InsertShare,
    InsertValidator,
    UpdateFeeRecipient,
    SetGraffiti,
    SetValidatorIndex,

    UpdateBlockNumber,
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
        SqlStatement::GetOperatorId,
        "SELECT operator_id FROM operators WHERE public_key = ?1",
    );
    m.insert(SqlStatement::GetAllOperators, "SELECT * FROM operators");
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
    m.insert(
        SqlStatement::GetAllClusters,
        "SELECT c.cluster_id, c.faulty, c.liquidated,
                v.validator_pubkey, v.fee_recipient, v.graffiti, v.validator_index, v.owner
         FROM clusters c
         JOIN cluster_members cm ON c.cluster_id = cm.cluster_id
         JOIN validators v ON c.cluster_id = v.cluster_id
         WHERE cm.operator_id = ?",
    );
    m.insert(
        SqlStatement::GetClusterMembers,
        "SELECT cm.cluster_id, cm.operator_id, s.share_pubkey, s.encrypted_key
         FROM cluster_members cm
         JOIN shares s ON cm.cluster_id = s.cluster_id AND cm.operator_id = s.operator_id
         WHERE cm.cluster_id = ?",
    );
    m.insert(SqlStatement::InsertShare,
        "INSERT INTO shares (validator_pubkey, cluster_id, operator_id, share_pubkey, encrypted_key) VALUES (?1, ?2, ?3, ?4, ?5)");
    m.insert(
        SqlStatement::InsertValidator,
        "INSERT INTO validators (validator_pubkey, cluster_id, fee_recipient, owner, validator_index) VALUES (?1, ?2, ?3, ?4, ?5)",
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
    m.insert(
        SqlStatement::UpdateBlockNumber,
        "UPDATE block SET block_number = 1?",
    );
    m
});
