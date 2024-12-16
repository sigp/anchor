use crate::{DatabaseError, NetworkDatabase, NetworkState, Pool, PoolConn, SqlStatement, SQL};
use base64::prelude::*;
use openssl::pkey::Public;
use openssl::rsa::Rsa;
use rusqlite::{params, OptionalExtension};
use ssv_types::{
    Cluster, ClusterId, ClusterMember, Operator, OperatorId, Share, ValidatorIndex,
    ValidatorMetadata,
};
use std::collections::{HashMap, HashSet};
use types::Address;

impl NetworkState {
    /// Build the network state from the database data
    pub(crate) fn new_with_state(
        conn_pool: &Pool,
        pubkey: &Rsa<Public>,
    ) -> Result<Self, DatabaseError> {
        // Get database connection from the pool
        let conn = conn_pool.get()?;

        // Get the last processed block from the database
        let last_processed_block = Self::get_last_processed_block(&conn)?;

        // Without an Id, we have no idea who we are. Check to see if an operator with our PublicKey
        // is stored the database, else we have to wait for it to be processed by the execution
        // layer
        let id = if let Ok(Some(operator_id)) = Self::does_self_exist(&conn, pubkey) {
            operator_id
        } else {
            // If it does not exist, just default the state
            return Ok(Self {
                last_processed_block,
                ..Default::default()
            });
        };

        // First Phase: Fetch data from the database
        // Get all of the operators from the network
        let operators = Self::fetch_operators(&conn)?;
        // Get clusters that this operator (id) participates in
        let clusters = Self::fetch_clusters(&conn, id)?;

        // Second phase: Transform data into efficient state stores
        // Pre-allocate HashMaps with known capacity
        let num_clusters = clusters.len();
        let mut shares: HashMap<ClusterId, Share> = HashMap::with_capacity(num_clusters);
        let mut validator_metadata: HashMap<ClusterId, ValidatorMetadata> =
            HashMap::with_capacity(num_clusters);
        let mut cluster_members: HashMap<ClusterId, HashSet<OperatorId>> =
            HashMap::with_capacity(num_clusters);

        // Populate state stores from cluster data
        clusters.iter().for_each(|cluster| {
            let cluster_id = cluster.cluster_id;

            // Store validator metadata for each cluster
            validator_metadata.insert(cluster_id, cluster.validator_metadata.to_owned());

            // Process each member in the cluster
            for member in cluster.cluster_members.clone().into_iter() {
                // Track cluster membership
                cluster_members
                    .entry(cluster_id)
                    .or_default()
                    .insert(member.operator_id);

                // If this member is us, store our share
                if member.operator_id == id {
                    shares.insert(cluster_id, member.share);
                }
            }
        });

        // Return fully constructed state
        Ok(Self {
            id: Some(id),
            operators,
            clusters: clusters.iter().map(|c| c.cluster_id).collect(),
            shares,
            validator_metadata,
            cluster_members,
            last_processed_block,
        })
    }

    // Get the last block that was processed and saved to db
    fn get_last_processed_block(conn: &PoolConn) -> Result<u64, DatabaseError> {
        conn.prepare_cached(SQL[&SqlStatement::GetBlockNumber])?
            .query_row(params![], |row| row.get(0))
            .map_err(DatabaseError::from)
    }

    // Check to see if an operator with the public key already exists in the database
    fn does_self_exist(
        conn: &PoolConn,
        pubkey: &Rsa<Public>,
    ) -> Result<Option<OperatorId>, DatabaseError> {
        let encoded = BASE64_STANDARD.encode(
            pubkey
                .public_key_to_pem()
                .expect("Failed to encode RsaPublicKey"),
        );
        let mut stmt = conn.prepare(SQL[&SqlStatement::GetOperatorId])?;
        stmt.query_row(params![encoded], |row| Ok(OperatorId(row.get(0)?)))
            .optional()
            .map_err(DatabaseError::from)
    }

    // Fetch and transform operator data from database
    fn fetch_operators(conn: &PoolConn) -> Result<HashMap<OperatorId, Operator>, DatabaseError> {
        let mut stmt = conn.prepare(SQL[&SqlStatement::GetAllOperators])?;
        let operators = stmt
            .query_map([], |row| {
                // Transform row into an operator and colleciton into HashMap
                let operator: Operator = row.try_into()?;
                Ok((operator.id, operator))
            })?
            .map(|result| result.map_err(DatabaseError::from));
        operators.collect()
    }

    // Fetch and transform cluster data for a specific operator
    fn fetch_clusters(
        conn: &PoolConn,
        operator_id: OperatorId,
    ) -> Result<Vec<Cluster>, DatabaseError> {
        let mut stmt = conn.prepare(SQL[&SqlStatement::GetAllClusters])?;
        let cluster = stmt
            .query_map([operator_id.0], |row| {
                let cluster_id = ClusterId(row.get(0)?);

                // Get all of the cluster members, and then construct the cluster
                let cluster_members = Self::fetch_cluster_members(conn, cluster_id)?;
                Cluster::try_from((row, cluster_members))
            })?
            .map(|result| result.map_err(DatabaseError::from));
        cluster.collect()
    }

    // Fetch members of a specific cluster
    fn fetch_cluster_members(
        conn: &PoolConn,
        cluster_id: ClusterId,
    ) -> Result<Vec<ClusterMember>, rusqlite::Error> {
        let mut stmt = conn.prepare(SQL[&SqlStatement::GetClusterMembers])?;
        let cluster_members = stmt.query_map([cluster_id.0], |row| {
            // Fetch all of the cluster members for the given ClusterId
            let share = row.try_into()?;
            Ok(ClusterMember {
                operator_id: OperatorId(row.get(1)?),
                cluster_id,
                share,
            })
        })?;
        cluster_members.collect()
    }
}

// Clean interface for accessing network state
impl NetworkDatabase {
    /// Get operator data from in-memory store
    pub fn get_operator(&self, id: &OperatorId) -> Option<Operator> {
        self.read_state(|state| state.operators.get(id).cloned())
    }

    /// Check if an operator exists
    pub fn operator_exists(&self, id: &OperatorId) -> bool {
        self.read_state(|state| state.operators.contains_key(id))
    }

    /// Check if a cluster exists
    pub fn cluster_exists(&self, id: &ClusterId) -> bool {
        self.read_state(|state| state.clusters.contains(id))
    }

    /// Check if we are a member of a specific cluster
    pub fn member_of_cluster(&self, id: &ClusterId) -> bool {
        self.read_state(|state| state.clusters.contains(id))
    }

    /// Get own share of key for a Cluster we are a member in
    pub fn get_share(&self, id: &ClusterId) -> Option<Share> {
        self.read_state(|state| state.shares.get(id).cloned())
    }

    /// Set the id of our own operator
    pub fn set_own_id(&self, id: OperatorId) {
        self.modify_state(|state| state.id = Some(id))
    }

    /// Get the metatdata for the cluster
    pub fn get_validator_metadata(&self, id: &ClusterId) -> Option<ValidatorMetadata> {
        self.read_state(|state| state.validator_metadata.get(id).cloned())
    }

    /// Get the last block that has been fully processed by the database
    pub fn get_last_processed_block(&self) -> u64 {
        self.read_state(|state| state.last_processed_block)
    }

    /// Get the Fee Recipient address
    pub fn get_fee_recipient(&self, id: &ClusterId) -> Option<Address> {
        self.read_state(|state| {
            state
                .validator_metadata
                .get(id)
                .map(|metadata| metadata.fee_recipient)
        })
    }

    /// Get the Validator Index
    pub fn get_validator_index(&self, id: &ClusterId) -> Option<ValidatorIndex> {
        self.read_state(|state| {
            state
                .validator_metadata
                .get(id)
                .map(|metadata| metadata.validator_index)
        })
    }
}
