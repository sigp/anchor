use crate::{ClusterMultiIndexMap, MetadataMultiIndexMap, MultiIndexMap, ShareMultiIndexMap};
use crate::{DatabaseError, NetworkDatabase, NetworkState, Pool, PoolConn};
use crate::{MultiState, SingleState};
use crate::{SqlStatement, SQL};
use base64::prelude::*;
use dashmap::{DashMap, DashSet};
use openssl::pkey::Public;
use openssl::rsa::Rsa;
use rusqlite::{params, OptionalExtension};
use ssv_types::{
    Cluster, ClusterId, ClusterMember, Operator, OperatorId, Share, ValidatorMetadata,
};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

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

        // Without an ID, we have no idea who we are. Check to see if an operator with our public key
        // is stored the database. If it does not exist, that means the operator still has to be registered
        // with the network contract or that we have not seen the corresponding event yet
        let id = if let Ok(Some(operator_id)) = Self::does_self_exist(&conn, pubkey) {
            operator_id
        } else {
            // If it does not exist, just default the state since we do not know who we are
            return Ok(Self {
                multi_state: MultiState {
                    shares: MultiIndexMap::default(),
                    validator_metadata: MultiIndexMap::default(),
                    clusters: MultiIndexMap::default(),
                },
                single_state: SingleState::default(),
            });
        };

        // First Phase: Fetch data from the database
        // Two main data structures for state reconstruction
        // 1) ClusterId ->  Cluster
        // 2) ClusterId -> Vec<(Share, ValidatorMetadata)>
        // This simplifies data reconstruction and makes it easy to add more customized stores in the future
        let operators = Self::fetch_operators(&conn)?;
        let share_validator = Self::fetch_shares_and_validators(&conn, id)?;
        let clusters = Self::fetch_clusters(&conn, id)?;

        // Second phase: Populate all in memory stores with data;
        let shares_multi: ShareMultiIndexMap = MultiIndexMap::new();
        let metadata_multi: MetadataMultiIndexMap = MultiIndexMap::new();
        let cluster_multi: ClusterMultiIndexMap = MultiIndexMap::new();
        let single_state = SingleState {
            id: AtomicU64::new(*id),
            last_processed_block: AtomicU64::new(last_processed_block),
            operators: DashMap::from_iter(operators),
            clusters: DashSet::from_iter(clusters.keys().copied()),
        };

        // Insert all of the cluster information
        clusters.iter().for_each(|(cluster_id, cluster)| {
            let validator_key = share_validator
                .get(cluster_id)
                .expect("Validator should exist")
                .1
                .public_key
                .clone();
            cluster_multi.insert(cluster_id, &validator_key, &cluster.owner, cluster.clone());
        });

        // Insert all of the share and validator_metadata
        share_validator
            .into_iter()
            .for_each(|(cluster_id, (share, metadata))| {
                let cluster_owner = clusters
                    .get(&cluster_id)
                    .expect("Cluster should exist")
                    .owner;
                shares_multi.insert(&metadata.public_key, &cluster_id, &cluster_owner, share);
                metadata_multi.insert(
                    &metadata.public_key,
                    &cluster_id,
                    &cluster_owner,
                    metadata.to_owned(),
                );
            });

        // Return fully constructed state
        Ok(Self {
            multi_state: MultiState {
                shares: shares_multi,
                validator_metadata: metadata_multi,
                clusters: cluster_multi,
            },
            single_state,
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

    // Fetch all of the validators and their associated share. Fetched together so that we can
    // guarantee that they pair up correctly
    fn fetch_shares_and_validators(
        conn: &PoolConn,
        operator_id: OperatorId,
    ) -> Result<HashMap<ClusterId, (Share, ValidatorMetadata)>, DatabaseError> {
        let mut stmt = conn.prepare(SQL[&SqlStatement::GetShareAndValidator])?;
        let data = stmt
            .query_map([*operator_id], |row| {
                let metadata = ValidatorMetadata::try_from(row)?;
                let share = Share::try_from(row)?;
                Ok((metadata.cluster_id, (share, metadata)))
            })?
            .map(|result| result.map_err(DatabaseError::from));
        data.collect::<Result<HashMap<_, _>, _>>()
    }

    // Fetch and transform cluster data for a specific operator
    fn fetch_clusters(
        conn: &PoolConn,
        operator_id: OperatorId,
    ) -> Result<HashMap<ClusterId, Cluster>, DatabaseError> {
        let mut stmt = conn.prepare(SQL[&SqlStatement::GetAllClusters])?;
        let clusters = stmt
            .query_map([*operator_id], |row| {
                let cluster_id = ClusterId(row.get(0)?);
                println!("got here");

                // Get all of the members for this cluster
                let cluster_members = Self::fetch_cluster_members(conn, cluster_id)?;

                // Convert row and members into cluster
                let cluster = Cluster::try_from((row, cluster_members))?;
                Ok((cluster_id, cluster))
            })?
            .map(|result| result.map_err(DatabaseError::from));
        clusters.collect::<Result<HashMap<_, _>, _>>()
    }

    // Fetch members of a specific cluster
    fn fetch_cluster_members(
        conn: &PoolConn,
        cluster_id: ClusterId,
    ) -> Result<Vec<ClusterMember>, rusqlite::Error> {
        let mut stmt = conn.prepare(SQL[&SqlStatement::GetClusterMembers])?;
        let members = stmt.query_map([cluster_id.0], |row| {
            Ok(ClusterMember {
                operator_id: OperatorId(row.get(0)?),
                cluster_id,
            })
        })?;

        members.collect()
    }
}

// Interface for accessing state data
impl NetworkDatabase {
    /// Get a reference to the shares map
    pub fn shares(&self) -> &ShareMultiIndexMap {
        &self.state.multi_state.shares
    }

    /// Get a reference to the validator metadata map
    pub fn metadata(&self) -> &MetadataMultiIndexMap {
        &self.state.multi_state.validator_metadata
    }

    /// Get a reference to the cluster map
    pub fn clusters(&self) -> &ClusterMultiIndexMap {
        &self.state.multi_state.clusters
    }

    /// Get the ID of our Operator if it exists
    pub fn get_own_id(&self) -> Option<OperatorId> {
        let id = self.state.single_state.id.load(Ordering::Relaxed);
        if id == u64::MAX {
            None
        } else {
            Some(OperatorId(id))
        }
    }

    /// Get operator data from in-memory store
    pub fn get_operator(&self, id: &OperatorId) -> Option<Operator> {
        self.state.single_state.operators.get(id).map(|v| v.clone())
    }

    /// Check if an operator exists
    pub fn operator_exists(&self, id: &OperatorId) -> bool {
        self.state.single_state.operators.contains_key(id)
    }

    /// Check if we are a member of a specific cluster
    pub fn member_of_cluster(&self, id: &ClusterId) -> bool {
        self.state.single_state.clusters.contains(id)
    }

    /// Get the last block that has been fully processed by the database
    pub fn get_last_processed_block(&self) -> u64 {
        self.state
            .single_state
            .last_processed_block
            .load(Ordering::Relaxed)
    }
}
