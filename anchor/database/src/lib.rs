use std::{
    collections::{HashMap, HashSet},
    error::Error,
    path::Path,
    str::FromStr,
    time::Duration,
};

use bls::PublicKeyBytes;
use once_cell::sync::OnceCell;
use openssl::{pkey::Public, rsa::Rsa};
use r2d2::CustomizeConnection;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::{Connection, Transaction, params};
use ssv_types::{Cluster, ClusterId, CommitteeId, Operator, OperatorId, Share, ValidatorMetadata};
use tokio::sync::{
    watch,
    watch::{Receiver, Ref},
};
use types::Address;

pub use crate::{
    error::DatabaseError,
    multi_index::{MultiIndexMap, *},
    slashing::SlashingProtection,
    state::{NetworkState, ProcessedEventCursor},
};

mod cluster_operations;
mod error;
mod keysplit_operations;
mod multi_index;
mod operator_operations;
mod schema;
mod share_operations;
pub mod slashing;
mod sql_operations;
mod state;
mod validator_operations;

// Compile tests module for crate tests or when the feature is enabled, but keep it private.
#[cfg(any(test, feature = "test-utils"))]
mod tests;

// Public, narrow re-export of just the test utilities when the feature is enabled.
#[cfg(feature = "test-utils")]
#[doc(hidden)]
pub mod test_utils {
    pub use super::{slashing::NoOpSlashingProtection, tests::utils::*};
}

const POOL_SIZE: u32 = 1;
const CONNECTION_TIMEOUT: Duration = Duration::from_secs(60);

type Pool = r2d2::Pool<SqliteConnectionManager>;
type PoolConn = r2d2::PooledConnection<SqliteConnectionManager>;

/// Parse a text column into a domain type while preserving the originating SQLite column index in
/// any conversion error.
pub(crate) fn parse_text_column<T>(row: &rusqlite::Row<'_>, column: usize) -> rusqlite::Result<T>
where
    T: FromStr,
    T::Err: Error + Send + Sync + 'static,
{
    let value = row.get::<_, String>(column)?;
    T::from_str(&value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            column,
            rusqlite::types::Type::Text,
            Box::new(error),
        )
    })
}

/// Like `parse_text_column`, but for nullable text columns that represent optional domain values.
pub(crate) fn parse_optional_text_column<T>(
    row: &rusqlite::Row<'_>,
    column: usize,
) -> rusqlite::Result<Option<T>>
where
    T: FromStr,
    T::Err: Error + Send + Sync + 'static,
{
    let value = row.get::<_, Option<String>>(column)?;
    value
        .map(|value| {
            T::from_str(&value).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    column,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })
        })
        .transpose()
}

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
    /// The last processed event inside a partially processed block.
    last_processed_event: Option<ProcessedEventCursor>,
    /// All of the operators in the network
    operators: HashMap<OperatorId, Operator>,
    /// All of the Clusters that we are a memeber of
    clusters: HashSet<ClusterId>,
    /// Nonce of the owner account
    nonces: HashMap<Address, u16>,
    /// Explicit fee recipient overrides keyed by owner address.
    fee_recipients: HashMap<Address, Address>,
    /// Monotonically increasing OperatorId count. None indicates a migrated database.
    max_operator_id_seen: Option<u64>,
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

#[derive(Clone, Copy)]
enum ProgressUpdate {
    None,
    Event(ProcessedEventCursor),
    Block(u64),
}

impl NetworkDatabase {
    /// Construct a new NetworkDatabase at the given path and the Public Key of the current operator
    pub fn new(
        path: &Path,
        pubkey: &Rsa<Public>,
        network_name: &str,
    ) -> Result<Self, DatabaseError> {
        let conn_pool = Self::open_or_create(path, network_name)?;
        let operator = PubkeyOrId::Pubkey(pubkey.clone());
        let state = watch::Sender::new(NetworkState::new_with_state(&conn_pool, &operator)?);
        Ok(Self {
            operator,
            state,
            conn_pool,
        })
    }

    /// Construct a new NetworkDatabase using an in-memory database (test-only)
    /// This is more explicit than passing ":memory:" as a path
    #[cfg(feature = "test-utils")]
    pub fn new_in_memory(pubkey: &Rsa<Public>, network_name: &str) -> Result<Self, DatabaseError> {
        let conn_pool = Self::open_in_memory(network_name)?;
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
        network_name: &str,
    ) -> Result<Self, DatabaseError> {
        let conn_pool = Self::open_or_create(path, network_name)?;
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

    /// Execute a short read against the latest committed state without letting the underlying
    /// `watch::Ref` escape into caller logic that may later publish another update.
    pub fn with_state<R>(&self, f: impl FnOnce(&NetworkState) -> R) -> R {
        let state = self.state.borrow();
        f(&state)
    }

    /// Subscribe to the committed in-memory view that is published after database commits.
    pub fn watch(&self) -> Receiver<NetworkState> {
        self.state.subscribe()
    }

    /// Persist only the exact last processed event cursor.
    ///
    /// This is used when an event succeeded, or was intentionally skipped, but the enclosing
    /// block/range has not yet been fully completed.
    ///
    /// The event cursor is a finer-grained form of progress than `last_processed_block`, so it
    /// must never point to a block that is older than the last fully processed block. Once a full
    /// block boundary is committed via `advance_processed_block`, the cursor is cleared again.
    pub fn mark_event_processed(&self, cursor: ProcessedEventCursor) -> Result<(), DatabaseError> {
        self.commit_db_update(ProgressUpdate::Event(cursor), false, |_| Ok(()), |_| {})
    }

    /// Collapse progress back to a fully processed block once the entire fetched range succeeded.
    ///
    /// This advances the coarse block boundary and clears any finer-grained event cursor, because
    /// there is no longer a partial block to resume within. Block progress is monotonic, so
    /// callers must only publish the current block boundary or a later one.
    pub fn advance_processed_block(&self, block_number: u64) -> Result<(), DatabaseError> {
        self.commit_db_update(
            ProgressUpdate::Block(block_number),
            false,
            |_| Ok(()),
            |_| {},
        )
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
        self.state.send_modify(|state| {
            state.single_state.last_processed_block = block_number;
            state.single_state.last_processed_event = None;
        });
        Ok(())
    }

    /// Update the largest seen OperatorId in the database
    pub fn set_max_operator_id_seen(
        &self,
        operator_id: u64,
        tx: &Transaction<'_>,
    ) -> Result<(), DatabaseError> {
        tx.prepare_cached(sql_operations::SET_MAX_OPERATOR_ID_SEEN)?
            .execute(params![operator_id])?;
        self.modify_state(|state| state.single_state.max_operator_id_seen = Some(operator_id));

        Ok(())
    }

    // Open an existing database at the given `path`, or create one if none exists.
    fn open_or_create(path: &Path, network_name: &str) -> Result<Pool, DatabaseError> {
        schema::ensure_up_to_date(path, network_name)?;
        Self::open_conn_pool(path)
    }

    // Build a new connection pool for file-based databases
    fn open_conn_pool(path: &Path) -> Result<Pool, DatabaseError> {
        let manager = SqliteConnectionManager::file(path);
        let conn_pool = Pool::builder()
            .max_size(POOL_SIZE)
            .connection_timeout(CONNECTION_TIMEOUT)
            .connection_customizer(Box::new(AnchorCustomizeConnection))
            .build(manager)?;
        Ok(conn_pool)
    }

    // Build a new connection pool for in-memory databases (test-only)
    // In-memory databases bypass schema migrations and are initialized via connection customizer
    #[cfg(feature = "test-utils")]
    fn open_in_memory(network_name: &str) -> Result<Pool, DatabaseError> {
        let manager = SqliteConnectionManager::memory();
        let conn_pool = Pool::builder()
            .max_size(POOL_SIZE)
            .connection_timeout(CONNECTION_TIMEOUT)
            .connection_customizer(Box::new(InMemoryCustomizeConnection {
                network_name: network_name.to_string(),
            }))
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

    /// Atomically apply a durable database change, persist the matching progress cursor, and only
    /// then publish the corresponding in-memory state update.
    ///
    /// If `notify` is `true`, `watch` subscribers are notified of the state change.
    /// Use `false` for internal bookkeeping (nonce bumps, cursor-only advances) that
    /// does not change observable validator/operator state.
    fn commit_db_update(
        &self,
        progress: ProgressUpdate,
        notify: bool,
        apply_tx: impl FnOnce(&Transaction<'_>) -> Result<(), DatabaseError>,
        apply_state: impl FnOnce(&mut NetworkState),
    ) -> Result<(), DatabaseError> {
        let mut conn = self.connection()?;
        let tx = conn.transaction()?;

        apply_tx(&tx)?;
        self.apply_progress_to_tx(progress, &tx)?;
        tx.commit()?;

        let publish = move |state: &mut NetworkState| {
            apply_state(state);
            self.apply_progress_to_state(progress, state);
        };

        if notify {
            self.state.send_modify(publish);
        } else {
            self.modify_state(publish);
        }

        Ok(())
    }

    fn apply_progress_to_tx(
        &self,
        progress: ProgressUpdate,
        tx: &Transaction<'_>,
    ) -> Result<(), DatabaseError> {
        match progress {
            ProgressUpdate::None => Ok(()),
            ProgressUpdate::Event(cursor) => tx
                .prepare_cached(sql_operations::SET_PROCESSED_EVENT_CURSOR)?
                .execute(params![
                    cursor.block_number,
                    cursor.transaction_index,
                    cursor.log_index
                ])
                .map(|_| ())
                .map_err(DatabaseError::from),
            ProgressUpdate::Block(block_number) => tx
                .prepare_cached(sql_operations::UPDATE_BLOCK_NUMBER)?
                .execute(params![block_number])
                .map(|_| ())
                .map_err(DatabaseError::from),
        }
    }

    fn apply_progress_to_state(&self, progress: ProgressUpdate, state: &mut NetworkState) {
        match progress {
            ProgressUpdate::None => {}
            ProgressUpdate::Event(cursor) => {
                debug_assert!(
                    cursor.block_number >= state.single_state.last_processed_block,
                    "processed event cursor must not point before the last fully processed block"
                );
                state.single_state.last_processed_event = Some(cursor);
            }
            ProgressUpdate::Block(block_number) => {
                debug_assert!(
                    block_number >= state.single_state.last_processed_block,
                    "fully processed block boundary must not regress"
                );
                state.single_state.last_processed_block = block_number;
                state.single_state.last_processed_event = None;
            }
        }
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

#[cfg(feature = "test-utils")]
#[derive(Debug)]
struct InMemoryCustomizeConnection {
    network_name: String,
}

#[cfg(feature = "test-utils")]
impl CustomizeConnection<Connection, rusqlite::Error> for InMemoryCustomizeConnection {
    fn on_acquire(&self, conn: &mut Connection) -> rusqlite::Result<()> {
        // For in-memory databases, create schema on each connection
        let _ = schema::create_initial_schema(conn, &self.network_name);
        Ok(())
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
