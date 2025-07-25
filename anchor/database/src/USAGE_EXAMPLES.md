# Database Component Usage Examples

## Basic Setup and Initialization

### Creating a New Database

```rust
use database::{NetworkDatabase, DatabaseError};
use std::path::Path;
use openssl::rsa::Rsa;
use openssl::pkey::Public;

// Create a new database with operator's RSA public key
let db_path = Path::new("network.db");
let operator_pubkey: Rsa<Public> = generate_rsa_keypair(); // Your RSA key generation
let db = NetworkDatabase::new(db_path, &operator_pubkey)?;
```

### Testing Setup (Impostor Mode)

```rust
use ssv_types::OperatorId;

// For testing: create database acting as specific operator
let operator_id = OperatorId(42);
let db = NetworkDatabase::new_as_impostor(db_path, &operator_id)?;
```

## Working with Transactions

All database operations require explicit transactions for consistency:

```rust
use rusqlite::Transaction;

// Standard transaction pattern
let mut conn = db.connection()?;
let tx = conn.transaction()?;

// Perform multiple operations...
db.insert_operator(&operator, &tx)?;
db.insert_validator(cluster, &validator, shares, &tx)?;

// Commit all changes atomically
tx.commit()?;
```

## Operator Management

### Adding Operators

```rust
use ssv_types::Operator;

let operator = Operator {
    id: OperatorId(1),
    public_key: operator_rsa_key,
    owner_address: eth_address,
};

let mut conn = db.connection()?;
let tx = conn.transaction()?;

db.insert_operator(&operator, &tx)?;
tx.commit()?;

// Verify operator was added
assert!(db.state().operator_exists(&operator.id));
```

### Retrieving Operators

```rust
// Get operator from in-memory state (fast)
let state = db.state();
if let Some(operator) = state.get_operator(&operator_id) {
    println!("Operator found: {:?}", operator);
}

// Check if operator exists
if state.operator_exists(&operator_id) {
    println!("Operator {} is registered", operator_id);
}
```

### Removing Operators

```rust
let mut conn = db.connection()?;
let tx = conn.transaction()?;

db.delete_operator(operator_id, &tx)?;
tx.commit()?;

// Verify deletion (also removes from all clusters and shares)
assert!(!db.state().operator_exists(&operator_id));
```

## Cluster and Validator Operations

### Adding Validators to Clusters

```rust
use ssv_types::{Cluster, ValidatorMetadata, Share, ClusterId};

// Create cluster configuration
let cluster = Cluster {
    cluster_id: ClusterId::from([1, 2, 3, 4]),
    cluster_members: operator_ids.into_iter().collect(),
    owner: cluster_owner_address,
    fee_recipient: fee_recipient_address,
    liquidated: false,
};

// Create validator metadata
let validator = ValidatorMetadata {
    public_key: validator_pubkey,
    cluster_id: cluster.cluster_id,
    index: Some(ValidatorIndex(123)),
    graffiti: [0u8; 32],
};

// Create encrypted shares for each operator
let shares: Vec<Share> = cluster.cluster_members
    .iter()
    .map(|&operator_id| Share {
        validator_pubkey: validator.public_key,
        cluster_id: cluster.cluster_id,
        operator_id,
        share_pubkey: Some(share_public_key),
        encrypted_key: Some(encrypted_key_data),
    })
    .collect();

// Insert complete validator setup
let mut conn = db.connection()?;
let tx = conn.transaction()?;

db.insert_validator(cluster.clone(), &validator, shares, &tx)?;
tx.commit()?;

// Verify insertion
assert!(db.state().member_of_cluster(&cluster.cluster_id));
```

### Removing Validators

```rust
let mut conn = db.connection()?;
let tx = conn.transaction()?;

// Delete validator (automatically cleans up cluster if last validator)
db.delete_validator(&validator_pubkey, &tx)?;
tx.commit()?;

// Verify complete cleanup
let state = db.state();
assert!(state.metadata().get_by(&validator_pubkey).is_none());
assert!(state.shares().get_by(&validator_pubkey).is_none());
```

## State Access Patterns

### Multi-Index Lookups

```rust
let state = db.state();

// Access shares by different indices
let shares_map = state.shares();

// By validator public key (primary index)
if let Some(share) = shares_map.get_by(&validator_pubkey) {
    println!("Share for validator: {:?}", share);
}

// By cluster ID (secondary index) - returns iterator
for share in shares_map.get_all_by(&cluster_id) {
    println!("Share in cluster: {:?}", share);
}

// By owner address (tertiary index) - returns iterator  
for share in shares_map.get_all_by(&owner_address) {
    println!("Share owned by: {:?}", share);
}

// Similar patterns work for clusters and metadata
let clusters_map = state.clusters();
let metadata_map = state.metadata();
```

### Checking Membership

```rust
let state = db.state();

// Check if we're a member of specific clusters
if state.member_of_cluster(&cluster_id) {
    println!("We are a member of cluster {}", cluster_id);
}

// Get all clusters we participate in
let our_clusters = state.get_own_clusters();
for cluster_id in our_clusters {
    println!("Member of cluster: {}", cluster_id);
}
```

## Fee Recipient Management

### Updating Fee Recipients

```rust
use types::Address;

let new_fee_recipient = Address::from_str("0x742d35Cc6634C0532925a3b8D24D6D4e4C123456")?;

let mut conn = db.connection()?;
let tx = conn.transaction()?;

db.update_fee_recipient(cluster_owner, new_fee_recipient, &tx)?;
tx.commit()?;
```

### Retrieving Fee Recipients

```rust
let mut conn = db.connection()?;
let tx = conn.transaction()?;

if let Some(fee_recipient) = db.fee_recipient_for_owner(&owner_address, &tx)? {
    println!("Fee recipient: {}", fee_recipient);
}
```

## Nonce Management

```rust
let mut conn = db.connection()?;
let tx = conn.transaction()?;

// Get current nonce and increment it atomically
let current_nonce = db.bump_and_get_nonce(&owner_address, &tx)?;
println!("Using nonce: {}", current_nonce);

tx.commit()?;
```

## Block Processing

```rust
// Update the last processed block number
let mut conn = db.connection()?;
let tx = conn.transaction()?;

db.processed_block(12345, &tx)?;
tx.commit()?;

// Check current sync status
let last_block = db.state().get_last_processed_block();
println!("Last processed block: {}", last_block);
```

## Reactive State Watching

```rust
use tokio::sync::watch::Receiver;

// Subscribe to state changes
let mut state_receiver: Receiver<NetworkState> = db.watch();

// React to state changes
tokio::spawn(async move {
    while state_receiver.changed().await.is_ok() {
        let state = state_receiver.borrow();
        println!("State updated! {} operators, {} clusters", 
                 state.operators.len(), 
                 state.get_own_clusters().len());
    }
});
```

## Key Retrieval Operations

```rust
// Get RSA public keys for specific operators
let operator_ids = vec![1, 2, 3, 4];
let public_keys = db.get_keys_for_operators(operator_ids)?;

for key in public_keys {
    println!("Retrieved key: {}", base64::encode(key.public_key_to_pem()?));
}

// Get nonce for owner (read-only)
if let Some(nonce) = db.get_nonce_for_owner(owner_address)? {
    println!("Current nonce for owner: {}", nonce);
}
```

## Error Handling

```rust
use database::DatabaseError;

match db.insert_operator(&operator, &tx) {
    Ok(()) => println!("Operator added successfully"),
    Err(DatabaseError::AlreadyPresent(msg)) => {
        println!("Operator already exists: {}", msg);
    },
    Err(DatabaseError::NotFound(msg)) => {
        println!("Required data not found: {}", msg);
    },
    Err(DatabaseError::SQLError(msg)) => {
        println!("Database error: {}", msg);
    },
    Err(e) => println!("Other error: {}", e),
}
```

## Committee Information

```rust
use ssv_types::{CommitteeId, CommitteeInfo};

let state = db.state();

// Get committee info by committee ID
if let Some(committee_info) = state.get_committee_info_by_committee_id(&committee_id) {
    println!("Committee members: {:?}", committee_info.committee_members);
    println!("Validator indices: {:?}", committee_info.validator_indices);
}

// Get committee info by validator public key
if let Some(committee_info) = state.get_committee_info_by_validator_pk(&validator_pubkey) {
    println!("Committee for validator: {:?}", committee_info);
}

// Get all validator indices we're responsible for
let our_indices = state.validator_indices();
println!("Our validator indices: {:?}", our_indices);
```

## Persistence and Recovery

```rust
// Database state automatically persists and survives restarts
{
    let db = NetworkDatabase::new(db_path, &pubkey)?;
    // ... perform operations ...
    // Database automatically saved
} // db dropped here

// Later: reconnect to existing database
let db = NetworkDatabase::new(db_path, &pubkey)?;
// All previous state is automatically restored from disk
assert!(db.state().operator_exists(&operator_id)); // Still there!
```

## Testing Patterns

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_database_operations() -> Result<(), DatabaseError> {
        // Use temporary directory for tests
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("test.db");
        
        let pubkey = generate_test_rsa_key();
        let db = NetworkDatabase::new(&db_path, &pubkey)?;
        
        // Test operations...
        let mut conn = db.connection()?;
        let tx = conn.transaction()?;
        
        // Your test code here...
        
        tx.commit()?;
        Ok(())
    }
}
```

## Best Practices

1. **Always use transactions** for database operations
2. **Prefer state() access** for reads (faster than database queries)
3. **Batch operations** within single transactions when possible
4. **Handle all error cases** - database operations can fail
5. **Use watch() for reactive patterns** instead of polling state
6. **Validate data** before database operations
7. **Use tempfile** for testing to avoid conflicts
8. **Commit transactions explicitly** - don't rely on auto-commit