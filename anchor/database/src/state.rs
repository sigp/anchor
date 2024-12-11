use crate::{DatabaseError, NetworkDatabase, NetworkState, Pool, PoolConn, SqlStatement, SQL};
use ssv_types::{
    Cluster, ClusterId, ClusterMember, Operator, OperatorId, Share, ValidatorMetadata,
};
use std::collections::{HashMap, HashSet};

impl NetworkState {
    // Main constructor that builds the network state from the database data
    pub(crate) fn new_with_state(conn_pool: &Pool, id: OperatorId) -> Result<Self, DatabaseError> {
        // Get database connection from the pool
        let conn = conn_pool.get()?;

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
        })
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
    pub fn get_operator(&self, id: &OperatorId) -> Option<&Operator> {
        self.state.operators.get(id)
    }

    /// Check if an operator exists
    pub fn operator_exists(&self, id: &OperatorId) -> bool {
        self.state.operators.contains_key(id)
    }

    /// Check if we are a member of a specific cluster
    pub fn member_of_cluster(&self, id: &ClusterId) -> bool {
        self.state.clusters.contains(id)
    }

    /// Set the id of our own operator
    pub fn set_own_id(&mut self, id: OperatorId) {
        self.state.id = Some(id);
    }
}
