# SSV Types Usage Examples

This document provides practical examples of how to use the SSV types library for common operations in distributed validator networks.

## Basic Type Creation

### Creating Operators

```rust
use ssv_types::{Operator, OperatorId};
use types::Address;

// Create an operator from PEM-encoded public key
let pem_data = b"LS0tLS1CRUdJTiBSU0EgUFVCTElDIEtFWS0tLS0t...";
let operator_id = OperatorId(1141);
let owner_address = Address::random();

let operator = Operator::new(pem_data, operator_id, owner_address)
    .expect("Valid PEM data should create operator successfully");

println!("Created operator with ID: {}", operator.id);
```

### Creating Clusters

```rust
use ssv_types::{Cluster, ClusterId, OperatorId};
use indexmap::IndexSet;
use types::Address;

// Create a cluster with multiple operators
let cluster_id = ClusterId([1u8; 32]);
let owner = Address::random();
let fee_recipient = Address::random();

let mut cluster_members = IndexSet::new();
cluster_members.insert(OperatorId(1));
cluster_members.insert(OperatorId(2));
cluster_members.insert(OperatorId(3));
cluster_members.insert(OperatorId(4));

let cluster = Cluster {
    cluster_id,
    owner,
    fee_recipient,
    liquidated: false,
    cluster_members,
};

// Calculate Byzantine fault tolerance
let max_faults = cluster.get_f();
println!("Cluster can tolerate {} faulty operators", max_faults);

// Get committee ID for this cluster
let committee_id = cluster.committee_id();
println!("Committee ID: {:?}", committee_id);
```

## Message Creation and Validation

### Creating SSV Messages

```rust
use ssv_types::message::{SSVMessage, MsgType, MessageId};

// Create a consensus message
let message_id = MessageId::from([0u8; 56]);
let consensus_data = vec![1, 2, 3, 4, 5]; // Serialized consensus data

let ssv_message = SSVMessage::new(
    MsgType::SSVConsensusMsgType,
    message_id,
    consensus_data,
).expect("Valid message should be created");

println!("Created SSV message of type: {:?}", ssv_message.msg_type());
```

### Creating Signed Messages

```rust
use ssv_types::{
    OperatorId,
    message::{SignedSSVMessage, SSVMessage, MsgType, MessageId, RSA_SIGNATURE_SIZE},
};

// Create the base SSV message
let ssv_message = SSVMessage::new(
    MsgType::SSVConsensusMsgType,
    MessageId::from([0u8; 56]),
    vec![1, 2, 3],
).unwrap();

// Create signatures (normally these would be actual RSA signatures)
let signatures = vec![
    vec![0u8; RSA_SIGNATURE_SIZE], // Signature from operator 1
    vec![1u8; RSA_SIGNATURE_SIZE], // Signature from operator 2
    vec![2u8; RSA_SIGNATURE_SIZE], // Signature from operator 3
];

// Operator IDs must be sorted
let operator_ids = vec![
    OperatorId(1),
    OperatorId(2),
    OperatorId(3),
];

let full_data = vec![4, 5, 6]; // Additional message data

let signed_message = SignedSSVMessage::new(
    signatures,
    operator_ids,
    ssv_message,
    full_data,
).expect("Valid signed message should be created");

println!("Created signed message with {} signatures", 
         signed_message.signatures().len());
```

### Message Aggregation

```rust
use ssv_types::message::{SignedSSVMessage, SSVMessage, MsgType, MessageId, RSA_SIGNATURE_SIZE};
use ssv_types::OperatorId;

// Create multiple single-signature messages
let base_message = SSVMessage::new(
    MsgType::SSVConsensusMsgType,
    MessageId::from([0u8; 56]),
    vec![1, 2, 3],
).unwrap();

let msg1 = SignedSSVMessage::new(
    vec![vec![0u8; RSA_SIGNATURE_SIZE]],
    vec![OperatorId(1)],
    base_message.clone(),
    vec![],
).unwrap();

let msg2 = SignedSSVMessage::new(
    vec![vec![1u8; RSA_SIGNATURE_SIZE]],
    vec![OperatorId(3)],
    base_message,
    vec![],
).unwrap();

// Aggregate messages
let mut aggregated = msg1;
aggregated.aggregate(std::iter::once(msg2));

// Signatures are automatically sorted by operator ID
println!("Aggregated message operator IDs: {:?}", aggregated.operator_ids());
// Output: [OperatorId(1), OperatorId(3)]
```

## Consensus Protocol Usage

### Creating QBFT Messages

```rust
use ssv_types::consensus::{QbftMessage, QbftMessageType};
use types::{Hash256, VariableList, typenum::U56};

let qbft_message = QbftMessage {
    qbft_message_type: QbftMessageType::Proposal,
    height: 100,
    round: 1,
    identifier: VariableList::from(vec![1u8; 56]),
    root: Hash256::random(),
    data_round: 1,
    round_change_justification: vec![],
    prepare_justification: vec![],
};

println!("QBFT message: {}", qbft_message);
```

### Creating Validator Duties

```rust
use ssv_types::consensus::{ValidatorDuty, BeaconRole, BEACON_ROLE_ATTESTER};
use ssv_types::ValidatorIndex;
use types::{PublicKeyBytes, Slot, CommitteeIndex, VariableList, typenum::U13};

let duty = ValidatorDuty {
    r#type: BEACON_ROLE_ATTESTER,
    pub_key: PublicKeyBytes::random(),
    slot: Slot::new(12345),
    validator_index: ValidatorIndex(42),
    committee_index: CommitteeIndex::new(1),
    committee_length: 128,
    committees_at_slot: 4,
    validator_committee_index: 10,
    validator_sync_committee_indices: VariableList::empty(),
};

println!("Validator {} has duty at slot {}", duty.validator_index.0, duty.slot);
```

### Creating Beacon Votes

```rust
use ssv_types::consensus::BeaconVote;
use types::{Hash256, Checkpoint, Epoch};

let beacon_vote = BeaconVote {
    block_root: Hash256::random(),
    source: Checkpoint {
        epoch: Epoch::new(10),
        root: Hash256::random(),
    },
    target: Checkpoint {
        epoch: Epoch::new(11),
        root: Hash256::random(),
    },
};

println!("Beacon vote from epoch {} to epoch {}", 
         beacon_vote.source.epoch, beacon_vote.target.epoch);
```

## Key Sharing Operations

### Creating Key Shares

```rust
use ssv_types::{Share, OperatorId, ClusterId, ENCRYPTED_KEY_LENGTH};
use types::PublicKeyBytes;

let share = Share {
    validator_pubkey: PublicKeyBytes::random(),
    operator_id: OperatorId(1),
    cluster_id: ClusterId([1u8; 32]),
    share_pubkey: PublicKeyBytes::random(),
    encrypted_private_key: [0u8; ENCRYPTED_KEY_LENGTH],
};

println!("Created key share for operator {} in cluster {:?}", 
         share.operator_id, share.cluster_id);
```

## Committee Management

### Creating Committee IDs

```rust
use ssv_types::{CommitteeId, OperatorId};

// Committee ID from operator list (will be sorted automatically)
let operators = vec![OperatorId(3), OperatorId(1), OperatorId(2)];
let committee_id = CommitteeId::from(operators);

// Committee ID from sorted slice
let sorted_operators = [OperatorId(1), OperatorId(2), OperatorId(3)];
let committee_id2 = CommitteeId::from(sorted_operators.as_slice());

// Both should be equal since the first one was sorted
assert_eq!(committee_id, committee_id2);
println!("Committee ID: {:?}", committee_id);
```

### Creating Committee Info

```rust
use ssv_types::{CommitteeInfo, OperatorId, ValidatorIndex};
use indexmap::IndexSet;

let mut committee_members = IndexSet::new();
committee_members.insert(OperatorId(1));
committee_members.insert(OperatorId(2));
committee_members.insert(OperatorId(3));

let validator_indices = vec![
    ValidatorIndex(100),
    ValidatorIndex(101),
    ValidatorIndex(102),
];

let committee_info = CommitteeInfo {
    committee_members,
    validator_indices,
};

println!("Committee has {} members serving {} validators",
         committee_info.committee_members.len(),
         committee_info.validator_indices.len());
```

## Serialization Examples

### SSZ Encoding/Decoding

```rust
use ssv_types::message::{SSVMessage, MsgType, MessageId};
use ssz::{Encode, Decode};

// Create and encode a message
let original = SSVMessage::new(
    MsgType::SSVConsensusMsgType,
    MessageId::from([42u8; 56]),
    vec![1, 2, 3, 4, 5],
).unwrap();

let encoded = original.as_ssz_bytes();
println!("Encoded message size: {} bytes", encoded.len());

// Decode the message
let decoded = SSVMessage::from_ssz_bytes(&encoded)
    .expect("Decoding should succeed");

assert_eq!(original, decoded);
println!("Successfully roundtrip encoded/decoded message");
```

## Error Handling Examples

### Message Validation Errors

```rust
use ssv_types::message::{SSVMessage, MsgType, MessageId, SSVMessageError};

// This will fail because data is empty
let result = SSVMessage::new(
    MsgType::SSVConsensusMsgType,
    MessageId::from([0u8; 56]),
    vec![], // Empty data
);

match result {
    Err(SSVMessageError::EmptyData) => {
        println!("Caught expected empty data error");
    }
    _ => panic!("Expected empty data error"),
}

// This will fail because data is too large
let huge_data = vec![0u8; 10_000_000]; // Much larger than allowed
let result = SSVMessage::new(
    MsgType::SSVConsensusMsgType,
    MessageId::from([0u8; 56]),
    huge_data,
);

match result {
    Err(SSVMessageError::SSVDataTooBig { got, max }) => {
        println!("Data too big: {} bytes, max allowed: {} bytes", got, max);
    }
    _ => panic!("Expected data too big error"),
}
```

### Signed Message Validation Errors

```rust
use ssv_types::{
    OperatorId,
    message::{SignedSSVMessage, SSVMessage, MsgType, MessageId, SignedSSVMessageError},
};

let ssv_msg = SSVMessage::new(
    MsgType::SSVConsensusMsgType,
    MessageId::from([0u8; 56]),
    vec![1, 2, 3],
).unwrap();

// This will fail because operator IDs are not sorted
let result = SignedSSVMessage::new(
    vec![vec![0u8; 256], vec![1u8; 256]],
    vec![OperatorId(2), OperatorId(1)], // Not sorted!
    ssv_msg,
    vec![],
);

match result {
    Err(SignedSSVMessageError::SignersNotSorted) => {
        println!("Caught expected signers not sorted error");
    }
    _ => panic!("Expected signers not sorted error"),
}
```

## Best Practices

### 1. Always Validate Input
```rust
// Always use the constructor methods which include validation
let message = SSVMessage::new(msg_type, msg_id, data)?;
// Don't create structs directly unless you're certain about validation
```

### 2. Handle Byzantine Fault Tolerance
```rust
// Ensure clusters have enough operators for fault tolerance
let cluster_size = cluster.cluster_members.len();
let max_faults = cluster.get_f();
if max_faults == 0 {
    eprintln!("Warning: Cluster has no fault tolerance!");
}
println!("Cluster of {} operators can tolerate {} faults", cluster_size, max_faults);
```

### 3. Maintain Operator Ordering
```rust
// Always keep operator IDs sorted for deterministic behavior
let mut operator_ids = vec![OperatorId(3), OperatorId(1), OperatorId(2)];
operator_ids.sort();
// Now use the sorted list
```

### 4. Use Type Safety
```rust
// Leverage the newtype patterns for type safety
fn process_cluster(cluster_id: ClusterId) { /* ... */ }
fn process_operator(operator_id: OperatorId) { /* ... */ }

// This won't compile - prevents mixing up IDs
// process_cluster(operator_id); // Error!
```

These examples demonstrate the key patterns for working with SSV types in a distributed validator environment. Always remember to handle errors appropriately and respect the validation constraints built into the type system.