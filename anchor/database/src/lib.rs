use dashmap::{DashMap, DashSet};
use openssl::{pkey::Public, rsa::Rsa};
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::params;
use ssv_types::{Cluster, ClusterId, Operator, OperatorId, Share, ValidatorMetadata};
use std::fs::File;
use std::path::Path;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::Duration;
use types::{Address, PublicKey};

pub use crate::error::DatabaseError;
pub use crate::multi_index::{MultiIndexMap, *};
use crate::sql_operations::{SqlStatement, SQL};

mod cluster_operations;
mod error;
mod multi_index;
mod operator_operations;
mod share_operations;
mod sql_operations;
mod state;
mod validator_operations;

#[cfg(test)]
mod tests;

const POOL_SIZE: u32 = 1;
const CONNECTION_TIMEOUT: Duration = Duration::from_secs(5);

type Pool = r2d2::Pool<SqliteConnectionManager>;
type PoolConn = r2d2::PooledConnection<SqliteConnectionManager>;

/// All of the shares that belong to the current operator
/// Primary: public key of validator. uniquely identifies share
/// Secondary: cluster id. corresponds to a list of shares
/// Tertiary: owner of the cluster. corresponds to a list of shares
pub(crate) type ShareMultiIndexMap =
    MultiIndexMap<PublicKey, ClusterId, Address, Share, NonUniqueTag, NonUniqueTag>;
/// Metadata for all validators in the network
/// Primary: public key of the validator. uniquely identifies the metadata
/// Secondary: cluster id. corresponds to list of metadata for all validators
/// Tertiary: owner of the cluster: corresponds to list of metadata for all validators
pub(crate) type MetadataMultiIndexMap =
    MultiIndexMap<PublicKey, ClusterId, Address, ValidatorMetadata, NonUniqueTag, NonUniqueTag>;
/// All of the clusters in the network
/// Primary: cluster id. uniquely identifies a cluster
/// Secondary: public key of the validator. uniquely identifies a cluster
/// Tertiary: owner of the cluster. uniquely identifies a cluster
pub(crate) type ClusterMultiIndexMap =
    MultiIndexMap<ClusterId, PublicKey, Address, Cluster, UniqueTag, UniqueTag>;

// Information that needs to be accessed via multiple different indicies
#[derive(Debug)]
struct MultiState {
    shares: ShareMultiIndexMap,
    validator_metadata: MetadataMultiIndexMap,
    clusters: ClusterMultiIndexMap,
}

// General information that can be single index access
#[derive(Debug, Default)]
struct SingleState {
    /// The ID of our own operator. This is determined via events when the operator is
    /// registered with the network. Therefore, this may not be available right away if the operator
    /// is running but has not been registered with the network contract yet.
    id: AtomicU64,
    /// The last block that was processed
    last_processed_block: AtomicU64,
    /// All of the operators in the network
    operators: DashMap<OperatorId, Operator>,
    /// All of the Clusters that we are a memeber of
    clusters: DashSet<ClusterId>,
    /// Nonce of the owner account
    nonces: DashMap<Address, u16>,
}

// Container to hold all network state
#[derive(Debug)]
struct NetworkState {
    multi_state: MultiState,
    single_state: SingleState,
}

/// Top level NetworkDatabase that contains in memory storage for quick access
/// to relevant information and a connection to the database
#[derive(Debug)]
pub struct NetworkDatabase {
    /// The public key of our operator
    pubkey: Rsa<Public>,
    /// Custom state stores for easy data access
    state: NetworkState,
    /// Connection to the database
    conn_pool: Pool,
}

impl NetworkDatabase {
    /// Construct a new NetworkDatabase at the given path and the Public Key of the current operator
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
    pub fn processed_block(&self, block_number: u64) -> Result<(), DatabaseError> {
        let conn = self.connection()?;
        conn.prepare_cached(SQL[&SqlStatement::UpdateBlockNumber])?
            .execute(params![block_number])?;
        self.state
            .single_state
            .last_processed_block
            .store(block_number, Ordering::Relaxed);
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
