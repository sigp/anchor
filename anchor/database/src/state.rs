use std::{
    collections::{HashMap, HashSet},
    str::FromStr,
};

use base64::prelude::*;
use openssl::{pkey::Public, rsa::Rsa};
use rusqlite::{params, types::Type, Error as SqlError, OptionalExtension};
use ssv_types::{
    Cluster, ClusterId, ClusterMember, CommitteeId, CommitteeInfo, IndexSet, Operator, OperatorId,
    Share, ValidatorIndex, ValidatorMetadata,
};
use types::{Address, PublicKeyBytes};

use crate::{
    ClusterMultiIndexMap, DatabaseError, MetadataMultiIndexMap, MultiIndexMap, MultiState,
    NonUniqueIndex, Pool, PoolConn, ShareMultiIndexMap, SingleState, SqlStatement, UniqueIndex,
    SQL,
};

// Container to hold all network state
#[derive(Debug)]
pub struct NetworkState {
    pub(crate) multi_state: MultiState,
    pub(crate) single_state: SingleState,
}

impl NetworkState {
    /// Build the network state from the database data
    pub(crate) fn new_with_state(
        conn_pool: &Pool,
        pubkey: &Rsa<Public>,
    ) -> Result<Self, DatabaseError> {
        // Get database connection from the pool
        let conn = conn_pool.get()?;

        // Get the last processed block from the database
        let last_processed_block = Self::get_last_processed_block_from_db(&conn)?;

        // Without an ID, we have no idea who we are. Check to see if an operator with our public
        // key is stored the database. If it does not exist, that means the operator still
        // has to be registered with the network contract or that we have not seen the
        // corresponding event yet
        let id = Self::does_self_exist(&conn, pubkey)?;

        // First Phase: Fetch data from the database
        // 1) OperatorId -> Operator
        let operators = Self::fetch_operators(&conn)?;
        // 2) ClusterId -> Cluster
        let cluster_map = Self::fetch_clusters(&conn)?;
        // 3) ClusterId -> Vec<ValidatorMetadata>
        let validator_map = Self::fetch_validators(&conn)?;
        // 4) ClusterId -> Vec<Share>
        let share_map = id.map(|id| Self::fetch_shares(&conn, id)).transpose()?;
        // 5) Owner -> Nonce (u16)
        let nonces = Self::fetch_nonces(&conn)?;

        // Second phase: Populate all in memory stores with data;
        let mut shares_multi: ShareMultiIndexMap = MultiIndexMap::new();
        let mut metadata_multi: MetadataMultiIndexMap = MultiIndexMap::new();
        let mut cluster_multi: ClusterMultiIndexMap = MultiIndexMap::new();
        let single_state = SingleState {
            id,
            last_processed_block,
            operators,
            clusters: share_map
                .as_ref()
                .map(|m| m.keys().copied().collect())
                .unwrap_or_default(),
            nonces,
        };

        // Populate all multi-index maps in a single pass through clusters
        for (cluster_id, cluster) in &cluster_map {
            // Get all the validator for this cluster
            let validators = validator_map
                .get(cluster_id)
                .expect("Validator for cluster must exist");

            // Process each validator and its associated data
            for validator in validators {
                // Insert cluster and validator metadata
                cluster_multi.insert(
                    cluster_id,
                    &validator.public_key,
                    &cluster.owner,
                    &cluster.committee_id(),
                    cluster.clone(),
                );
                metadata_multi.insert(
                    &validator.public_key,
                    cluster_id,
                    &cluster.owner,
                    &cluster.committee_id(),
                    validator.clone(),
                );

                // Process this validators shares
                if let Some(share_map) = &share_map {
                    if let Some(shares) = share_map.get(cluster_id) {
                        for share in shares {
                            if share.validator_pubkey == validator.public_key {
                                shares_multi.insert(
                                    &validator.public_key,
                                    cluster_id,
                                    &cluster.owner,
                                    &cluster.committee_id(),
                                    share.clone(),
                                );
                            }
                        }
                    }
                }
            }
        }

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
    fn get_last_processed_block_from_db(conn: &PoolConn) -> Result<u64, DatabaseError> {
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
                // Transform row into an operator and collect into HashMap
                let operator: Operator = row.try_into()?;
                Ok((operator.id, operator))
            })?
            .map(|result| result.map_err(DatabaseError::from));
        operators.collect()
    }

    // Fetch and transform validator data from the database
    fn fetch_validators(
        conn: &PoolConn,
    ) -> Result<HashMap<ClusterId, Vec<ValidatorMetadata>>, DatabaseError> {
        let mut stmt = conn.prepare(SQL[&SqlStatement::GetAllValidators])?;
        let validators = stmt
            .query_map([], |row| ValidatorMetadata::try_from(row))?
            .map(|result| result.map_err(DatabaseError::from))
            .collect::<Result<Vec<_>, _>>()?;

        let mut map = HashMap::new();
        for validator in validators {
            map.entry(validator.cluster_id)
                .or_insert_with(Vec::new)
                .push(validator);
        }
        Ok(map)
    }

    // Fetch and transform cluster data from the database
    fn fetch_clusters(conn: &PoolConn) -> Result<HashMap<ClusterId, Cluster>, DatabaseError> {
        let mut stmt = conn.prepare(SQL[&SqlStatement::GetAllClusters])?;
        let clusters = stmt
            .query_map([], |row| {
                let cluster_id = ClusterId(row.get(0)?);

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

    // Fetch the shares for a specific operator
    fn fetch_shares(
        conn: &PoolConn,
        id: OperatorId,
    ) -> Result<HashMap<ClusterId, Vec<Share>>, DatabaseError> {
        let mut stmt = conn.prepare(SQL[&SqlStatement::GetShares])?;
        let shares = stmt
            .query_map([*id], |row| Share::try_from(row))?
            .map(|result| result.map_err(DatabaseError::from))
            .collect::<Result<Vec<_>, _>>()?;

        let mut map = HashMap::new();
        for share in shares {
            map.entry(share.cluster_id)
                .or_insert_with(Vec::new)
                .push(share);
        }
        Ok(map)
    }

    // Fetch all of the owner nonce pairs
    fn fetch_nonces(conn: &PoolConn) -> Result<HashMap<Address, u16>, DatabaseError> {
        let mut stmt = conn.prepare(SQL[&SqlStatement::GetAllNonces])?;
        let nonces = stmt
            .query_map([], |row| {
                // Get the owner from column 0
                let owner_str = row.get::<_, String>(0)?;
                let owner = Address::from_str(&owner_str)
                    .map_err(|e| SqlError::FromSqlConversionFailure(1, Type::Text, Box::new(e)))?;

                // Get he nonce from column 1
                let nonce = row.get::<_, u16>(1)?;
                Ok((owner, nonce))
            })?
            .map(|result| result.map_err(DatabaseError::from));
        nonces.collect()
    }

    fn get_cluster_members(&self, committee_id: &CommitteeId) -> Option<IndexSet<OperatorId>> {
        self.multi_state
            .clusters
            .get_all_by(committee_id)
            .and_then(|clusters| clusters.first().cloned())
            .map(|cluster| cluster.cluster_members)
    }

    fn get_cluster_members_for_validator(
        &self,
        validator_pk: &PublicKeyBytes,
    ) -> Option<IndexSet<OperatorId>> {
        let cluster_id = self
            .multi_state
            .validator_metadata
            .get_by(validator_pk)
            .map(|v| v.cluster_id)?;
        self.multi_state
            .clusters
            .get_by(&cluster_id)
            .map(|c| c.cluster_members)
    }

    fn get_validator_indices(&self, committee_id: &CommitteeId) -> Option<Vec<ValidatorIndex>> {
        self.multi_state
            .validator_metadata
            .get_all_by(committee_id)
            .map(|metadata| {
                metadata
                    .iter()
                    .map(|metadata| metadata.index)
                    .collect::<Vec<_>>()
            })
    }

    /// Get a reference to the shares map
    pub fn shares(&self) -> &ShareMultiIndexMap {
        &self.multi_state.shares
    }

    /// Get a reference to the validator metadata map
    pub fn metadata(&self) -> &MetadataMultiIndexMap {
        &self.multi_state.validator_metadata
    }

    /// Get a reference to the cluster map
    pub fn clusters(&self) -> &ClusterMultiIndexMap {
        &self.multi_state.clusters
    }

    /// Get the ID of our Operator if it exists
    pub fn get_own_id(&self) -> Option<OperatorId> {
        self.single_state.id
    }

    /// Get operator data from in-memory store
    pub fn get_operator(&self, id: &OperatorId) -> Option<Operator> {
        self.single_state.operators.get(id).cloned()
    }

    /// Check if an operator exists
    pub fn operator_exists(&self, id: &OperatorId) -> bool {
        self.single_state.operators.contains_key(id)
    }

    /// Check if we are a member of a specific cluster
    pub fn member_of_cluster(&self, id: &ClusterId) -> bool {
        self.single_state.clusters.contains(id)
    }

    /// Get the clusters we are member of
    pub fn get_own_clusters(&self) -> &HashSet<ClusterId> {
        &self.single_state.clusters
    }

    /// Get the last block that has been fully processed by the database
    pub fn get_last_processed_block(&self) -> u64 {
        self.single_state.last_processed_block
    }

    pub fn get_committee_info_by_committee_id(
        &self,
        committee_id: &CommitteeId,
    ) -> Option<CommitteeInfo> {
        // Get committee members
        let committee_members = self.get_cluster_members(committee_id)?;

        // Get validator indices for this committee
        let validator_indices = self.get_validator_indices(committee_id)?;

        Some(CommitteeInfo {
            committee_members,
            validator_indices,
        })
    }

    pub fn get_committee_info_by_validator_pk(
        &self,
        validator_pk: &PublicKeyBytes,
    ) -> Option<CommitteeInfo> {
        let validator_index = self
            .multi_state
            .validator_metadata
            .get_by(validator_pk)
            .map(|v| v.index)?;

        let committee_members = self.get_cluster_members_for_validator(validator_pk)?;

        Some(CommitteeInfo {
            committee_members,
            validator_indices: vec![validator_index],
        })
    }
}
