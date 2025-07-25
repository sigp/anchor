# Database Component Technical Specification

## Module Structure

```
database/
├── src/
│   ├── lib.rs                    # Main module, NetworkDatabase struct
│   ├── state.rs                  # NetworkState and state management
│   ├── error.rs                  # DatabaseError types
│   ├── multi_index.rs            # MultiIndexMap implementation
│   ├── sql_operations.rs         # SQL query constants
│   ├── cluster_operations.rs     # Cluster CRUD operations
│   ├── operator_operations.rs    # Operator CRUD operations
│   ├── validator_operations.rs   # Validator CRUD operations
│   ├── share_operations.rs       # Share CRUD operations
│   ├── keysplit_operations.rs    # Key retrieval operations
│   ├── table_schema.sql          # Database schema definition
│   └── tests/                    # Integration and unit tests
└── Cargo.toml
```

## Core Data Structures

### NetworkDatabase
```rust
pub struct NetworkDatabase {
    operator: PubkeyOrId,                    // Operator identity (pubkey or ID)
    state: watch::Sender<NetworkState>,     // State management with notifications
    conn_pool: Pool,                        // SQLite connection pool
}
```

### NetworkState
```rust
pub struct NetworkState {
    multi_state: MultiState,     // Multi-indexed entity maps
    single_state: SingleState,   // Simple key-value storage
}
```

### MultiState
```rust
struct MultiState {
    shares: ShareMultiIndexMap,           // All operator shares
    validator_metadata: MetadataMultiIndexMap, // Validator information
    clusters: ClusterMultiIndexMap,       // Cluster configurations
}
```

### SingleState
```rust
struct SingleState {
    id: Option<OperatorId>,               // Current operator ID
    last_processed_block: u64,            // Sync state
    operators: HashMap<OperatorId, Operator>, // All network operators
    clusters: HashSet<ClusterId>,         // Clusters we participate in
    nonces: HashMap<Address, u16>,        // Owner transaction nonces
}
```

## Type Aliases

```rust
// Multi-index map for shares: indexed by validator pubkey, cluster ID, owner
pub type ShareMultiIndexMap = MultiIndexMap<
    PublicKeyBytes,    // Primary: validator public key
    ClusterId,         // Secondary: cluster ID
    Address,           // Tertiary: cluster owner
    CommitteeId,       // Quaternary: committee ID
    Share,             // Value type
    NonUniqueTag, NonUniqueTag, NonUniqueTag  // Index uniqueness
>;

// Similar structure for validator metadata and clusters
pub type MetadataMultiIndexMap = MultiIndexMap<...>;
pub type ClusterMultiIndexMap = MultiIndexMap<...>;
```

## Database Schema

### Tables
- **block**: Tracks last processed block number
- **owners**: Cluster owners with fee recipients and nonces
- **operators**: Network operators with RSA public keys
- **clusters**: Validator clusters with liquidation status
- **cluster_members**: Many-to-many operator-cluster relationships
- **validators**: Validator metadata including indices and graffiti
- **shares**: Encrypted key shares distributed among operators

### Relationships
- Clusters → Validators (1:N)
- Validators → Shares (1:N) 
- Operators ↔ Clusters (N:M via cluster_members)
- Shares → Operators (N:1)

## API Specification

### Database Management
```rust
impl NetworkDatabase {
    pub fn new(path: &Path, pubkey: &Rsa<Public>) -> Result<Self, DatabaseError>
    pub fn new_as_impostor(path: &Path, operator: &OperatorId) -> Result<Self, DatabaseError>
    pub fn state(&self) -> Ref<'_, NetworkState>
    pub fn watch(&self) -> Receiver<NetworkState>
    pub fn connection(&self) -> Result<PoolConn, DatabaseError>
    pub fn processed_block(&self, block_number: u64, tx: &Transaction) -> Result<(), DatabaseError>
}
```

### Cluster Operations
```rust
impl NetworkDatabase {
    pub fn insert_validator(&self, cluster: Cluster, validator: &ValidatorMetadata, 
                           shares: Vec<Share>, tx: &Transaction) -> Result<(), DatabaseError>
    pub fn update_status(&self, cluster_id: ClusterId, status: bool, 
                        tx: &Transaction) -> Result<(), DatabaseError>
    pub fn delete_validator(&self, validator_pubkey: &PublicKeyBytes, 
                          tx: &Transaction) -> Result<(), DatabaseError>
    pub fn bump_and_get_nonce(&self, owner: &Address, 
                             tx: &Transaction) -> Result<u16, DatabaseError>
}
```

### Operator Operations
```rust
impl NetworkDatabase {
    pub fn insert_operator(&self, operator: &Operator, 
                          tx: &Transaction) -> Result<(), DatabaseError>
    pub fn delete_operator(&self, id: OperatorId, 
                          tx: &Transaction) -> Result<(), DatabaseError>
}
```

### Validator Operations
```rust
impl NetworkDatabase {
    pub fn update_fee_recipient(&self, owner: Address, fee_recipient: Address, 
                               tx: &Transaction) -> Result<(), DatabaseError>
    pub fn fee_recipient_for_owner(&self, owner: &Address, 
                                  tx: &Transaction) -> Result<Option<Address>, DatabaseError>
    pub fn update_graffiti(&self, validator_pubkey: &PublicKeyBytes, graffiti: Graffiti, 
                          tx: &Transaction) -> Result<(), DatabaseError>
    pub fn set_validator_indices(&self, map: HashMap<PublicKeyBytes, ValidatorIndex>)
}
```

### Key Operations
```rust
impl NetworkDatabase {
    pub fn get_keys_for_operators(&self, operators: Vec<u64>) -> Result<Vec<Rsa<Public>>, DatabaseError>
    pub fn get_nonce_for_owner(&self, owner: Address) -> Result<Option<u16>, DatabaseError>
}
```

### State Access Methods
```rust
impl NetworkState {
    pub fn shares(&self) -> &ShareMultiIndexMap
    pub fn metadata(&self) -> &MetadataMultiIndexMap  
    pub fn clusters(&self) -> &ClusterMultiIndexMap
    pub fn get_own_id(&self) -> Option<OperatorId>
    pub fn get_operator(&self, id: &OperatorId) -> Option<Operator>
    pub fn operator_exists(&self, id: &OperatorId) -> bool
    pub fn member_of_cluster(&self, id: &ClusterId) -> bool
    pub fn get_own_clusters(&self) -> &HashSet<ClusterId>
    pub fn get_last_processed_block(&self) -> u64
    pub fn get_committee_info_by_committee_id(&self, committee_id: &CommitteeId) -> Option<CommitteeInfo>
    pub fn get_committee_info_by_validator_pk(&self, validator_pk: &PublicKeyBytes) -> Option<CommitteeInfo>
    pub fn validator_indices(&self) -> Vec<u64>
}
```

## Error Types

```rust
pub enum DatabaseError {
    NotFound(String),           // Entity not found
    AlreadyPresent(String),     // Duplicate entity
    IOError(ErrorKind),         // File system errors
    SQLError(String),           // SQL operation failures
    SQLPoolError(String),       // Connection pool errors
}
```

## Configuration Constants

```rust
const POOL_SIZE: u32 = 1;                                    // Single connection
const CONNECTION_TIMEOUT: Duration = Duration::from_secs(5); // 5 second timeout
```

## Dependencies

### Core Dependencies
- `rusqlite = "0.31"` - SQLite interface
- `r2d2 = "0.8"` - Connection pooling  
- `r2d2_sqlite = "0.22"` - SQLite pool adapter
- `openssl = "0.10"` - RSA key operations
- `tokio = "1.0"` - Async runtime and sync primitives
- `once_cell = "1.0"` - Lazy static initialization

### Internal Dependencies  
- `ssv_types` - SSV-specific data types
- `types` - Common type definitions

## Thread Safety

- `NetworkDatabase` is `Send + Sync` due to connection pooling
- State updates use `watch::Sender` for thread-safe notifications
- All database operations require explicit transactions for consistency

## Performance Characteristics

- **In-Memory Caching**: Critical data cached in MultiIndexMaps
- **Connection Pooling**: Single connection pool to avoid contention
- **Multi-Index Lookups**: O(1) access by any index key
- **Watch Notifications**: Efficient state change propagation
- **Batch Operations**: Transaction support for atomic multi-operation updates