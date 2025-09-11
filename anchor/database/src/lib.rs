use std::{
    collections::{HashMap, HashSet},
    path::Path,
    time::Duration,
};

use once_cell::sync::OnceCell;
use openssl::{pkey::Public, rsa::Rsa};
use r2d2::CustomizeConnection;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::{Connection, Transaction, params};
use ssv_types::{
    Cluster, ClusterId, CommitteeId, Operator, OperatorId, Share, ValidatorMetadata,
    domain_type::DomainType,
};
use tokio::sync::{
    watch,
    watch::{Receiver, Ref},
};
use types::{Address, PublicKeyBytes};

pub use crate::{
    error::DatabaseError,
    multi_index::{MultiIndexMap, *},
    state::NetworkState,
};

mod cluster_operations;
mod error;
mod keysplit_operations;
mod multi_index;
mod operator_operations;
mod schema;
mod share_operations;
mod sql_operations;
mod state;
mod validator_operations;

#[cfg(test)]
mod tests;

const POOL_SIZE: u32 = 1;
const CONNECTION_TIMEOUT: Duration = Duration::from_secs(60);

type Pool = r2d2::Pool<SqliteConnectionManager>;
type PoolConn = r2d2::PooledConnection<SqliteConnectionManager>;

/// All the shares that belong to the current operator.
/// IMPORTANT: There are parts of the code that assume this only contains shares that belong to the
/// current operator. If this ever changes, make sure to update the code accordingly.
/// Primary: public key of validator, uniquely identifies a share
/// Secondary: cluster id, corresponds to a list of shares
/// Tertiary: owner of the cluster, corresponds to a list of shares
pub type ShareMultiIndexMap = MultiIndexMap<
    PublicKeyBytes,
    ClusterId,
    Address,
    CommitteeId,
    Share,
    NonUniqueTag,
    NonUniqueTag,
    NonUniqueTag,
>;
/// Metadata for all validators in the network
/// Primary: public key of the validator. uniquely identifies the metadata
/// Secondary: cluster id. corresponds to list of metadata for all validators
/// Tertiary: owner of the cluster: corresponds to list of metadata for all validators
pub type MetadataMultiIndexMap = MultiIndexMap<
    PublicKeyBytes,
    ClusterId,
    Address,
    CommitteeId,
    ValidatorMetadata,
    NonUniqueTag,
    NonUniqueTag,
    NonUniqueTag,
>;
/// All of the clusters in the network
/// Primary: cluster id. uniquely identifies a cluster
/// Secondary: public key of the validator. uniquely identifies a cluster
/// Tertiary: owner of the cluster. does not uniquely identify a cluster
pub type ClusterMultiIndexMap = MultiIndexMap<
    ClusterId,
    PublicKeyBytes,
    Address,
    CommitteeId,
    Cluster,
    UniqueTag,
    NonUniqueTag,
    NonUniqueTag,
>;

// Information that needs to be accessed via multiple different indicies
#[derive(Debug)]
struct MultiState {
    shares: ShareMultiIndexMap,
    validator_metadata: MetadataMultiIndexMap,
    clusters: ClusterMultiIndexMap,
    // Be careful when adding new maps here. If you really must to, it must be updated in the
    // operations files
}

// General information that can be single index access
#[derive(Debug, Default)]
struct SingleState {
    /// The ID of our own operator. This is determined via events when the operator is
    /// registered with the network. Therefore, this may not be available right away if the
    /// operator is running but has not been registered with the network contract yet.
    id: Option<OperatorId>,
    /// The last block that was processed
    last_processed_block: u64,
    /// All of the operators in the network
    operators: HashMap<OperatorId, Operator>,
    /// All of the Clusters that we are a memeber of
    clusters: HashSet<ClusterId>,
    /// Nonce of the owner account
    nonces: HashMap<Address, u16>,
}

#[derive(Debug)]
enum PubkeyOrId {
    Pubkey(Rsa<Public>),
    Id(OperatorId),
}

/// Top level NetworkDatabase that contains in memory storage for quick access
/// to relevant information and a connection to the database
#[derive(Debug)]
pub struct NetworkDatabase {
    /// The public key or ID of our operator
    operator: PubkeyOrId,
    /// Custom state stores for easy data access
    state: watch::Sender<NetworkState>,
    /// Connection to the database
    conn_pool: Pool,
}

impl NetworkDatabase {
    /// Construct a new NetworkDatabase at the given path and the Public Key of the current operator
    pub fn new(
        path: &Path,
        pubkey: &Rsa<Public>,
        domain: DomainType,
    ) -> Result<Self, DatabaseError> {
        let conn_pool = Self::open_or_create(path, domain)?;
        let operator = PubkeyOrId::Pubkey(pubkey.clone());
        let state = watch::Sender::new(NetworkState::new_with_state(&conn_pool, &operator)?);
        Ok(Self {
            operator,
            state,
            conn_pool,
        })
    }

    /// Act as if we had the pubkey of a certain operator
    pub fn new_as_impostor(
        path: &Path,
        operator: &OperatorId,
        domain: DomainType,
    ) -> Result<Self, DatabaseError> {
        let conn_pool = Self::open_or_create(path, domain)?;
        let operator = PubkeyOrId::Id(*operator);
        let state = watch::Sender::new(NetworkState::new_with_state(&conn_pool, &operator)?);
        Ok(Self {
            operator,
            state,
            conn_pool,
        })
    }

    pub fn state(&self) -> Ref<'_, NetworkState> {
        self.state.borrow()
    }

    pub fn watch(&self) -> Receiver<NetworkState> {
        self.state.subscribe()
    }

    /// Update the last processed block number in the database
    /// Also, trigger a notification for other code to act on the new state
    pub fn processed_block(
        &self,
        block_number: u64,
        tx: &Transaction<'_>,
    ) -> Result<(), DatabaseError> {
        tx.prepare_cached(sql_operations::UPDATE_BLOCK_NUMBER)?
            .execute(params![block_number])?;
        self.state
            .send_modify(|state| state.single_state.last_processed_block = block_number);
        Ok(())
    }

    // Open an existing database at the given `path`, or create one if none exists.
    fn open_or_create(path: &Path, domain: DomainType) -> Result<Pool, DatabaseError> {
        schema::ensure_up_to_date(path, domain)?;
        Self::open_conn_pool(path)
    }

    // Build a new connection pool
    fn open_conn_pool(path: &Path) -> Result<Pool, DatabaseError> {
        let manager = SqliteConnectionManager::file(path);
        // some other args here
        let conn_pool = Pool::builder()
            .max_size(POOL_SIZE)
            .connection_timeout(CONNECTION_TIMEOUT)
            .connection_customizer(Box::new(AnchorCustomizeConnection))
            .build(manager)?;
        Ok(conn_pool)
    }

    // Open a new connection
    pub fn connection(&self) -> Result<PoolConn, DatabaseError> {
        Ok(self.conn_pool.get()?)
    }

    /// for convenience: Apply a modification to the state without triggering a notification
    /// This will be done at the end of a block via `processed_block` to avoid spamming
    fn modify_state(&self, f: impl FnOnce(&mut NetworkState)) {
        self.state.send_if_modified(|state| {
            f(state);
            false
        });
    }
}

#[derive(Debug)]
struct AnchorCustomizeConnection;

impl CustomizeConnection<Connection, rusqlite::Error> for AnchorCustomizeConnection {
    fn on_acquire(&self, conn: &mut Connection) -> rusqlite::Result<()> {
        conn.pragma_update(None, "journal_mode", "wal")?;
        conn.pragma_update(None, "locking_mode", "exclusive")
    }
}

/// A helper to get the operator ID of the current operator. Caches the ID after successfully
/// retrieving it to avoid locking the state further.
#[derive(Clone)]
pub enum OwnOperatorId {
    /// The operator ID was known when the `OwnOperatorId` was created.
    Known(OperatorId),
    /// The operator ID was not known when the `OwnOperatorId` was created. It will be retrieved
    /// from the `receiver` and cached in the `id` on first success.
    FromState {
        receiver: Receiver<NetworkState>,
        /// We use a `OnceLock` so that `get` can be called without a mutable reference.
        id: OnceCell<OperatorId>,
    },
}

impl OwnOperatorId {
    /// Creates the `OwnOperatorId` to either immediately store the operator ID or to recheck it on
    /// later `get` calls.
    pub fn new(receiver: Receiver<NetworkState>) -> Self {
        if let Some(operator_id) = receiver.borrow().get_own_id() {
            Self::Known(operator_id)
        } else {
            Self::FromState {
                receiver,
                id: OnceCell::new(),
            }
        }
    }

    /// Get the operator ID if it is available. Caches the ID internally after the first successful
    /// call to avoid locking the state in the future. This is possible because the own Operator ID
    /// never changes.
    pub fn get(&self) -> Option<OperatorId> {
        match self {
            Self::Known(id) => Some(*id),
            Self::FromState { receiver, id } => {
                // Switch to `std`'s OnceLock as soon as `get_or_try_init` is stable
                id.get_or_try_init(|| receiver.borrow().get_own_id().ok_or(()))
                    .ok()
                    .copied()
            }
        }
    }
}

impl From<OperatorId> for OwnOperatorId {
    fn from(operator_id: OperatorId) -> Self {
        Self::Known(operator_id)
    }
}
