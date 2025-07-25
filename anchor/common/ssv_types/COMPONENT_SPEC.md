# SSV Types Component Specification

## Module Specifications

### 1. Cluster Module (`cluster.rs`)

#### `ClusterId`
```rust
pub struct ClusterId(pub [u8; 32]);
```
- **Purpose**: Unique 32-byte identifier for clusters
- **Implementation**: Newtype wrapper around byte array
- **Traits**: `Clone, Copy, Default, Eq, PartialEq, Hash, From, Deref`
- **Display**: Hex-encoded string representation

#### `Cluster`
```rust
pub struct Cluster {
    pub cluster_id: ClusterId,
    pub owner: Address,
    pub fee_recipient: Address,
    pub liquidated: bool,
    pub cluster_members: IndexSet<OperatorId>,
}
```
- **Purpose**: Represents a group of operators serving validators
- **Key Methods**:
  - `get_f() -> u64`: Returns maximum tolerable faulty members `(n-1)/3`
  - `committee_id() -> CommitteeId`: Derives committee ID from members
- **Constraints**: 
  - Must have at least 1 member for `get_f()` to return > 0
  - Byzantine fault tolerance requires `3f+1` members

#### `ValidatorIndex`
```rust
pub struct ValidatorIndex(pub usize);
```
- **Purpose**: Index of validator in registry
- **Traits**: SSZ encodable/decodable with transparent behavior
- **Conversions**: `From<ValidatorIndex> for u64`

#### `ValidatorMetadata`
```rust
pub struct ValidatorMetadata {
    pub public_key: PublicKeyBytes,
    pub cluster_id: ClusterId,
    pub index: Option<ValidatorIndex>,
    pub graffiti: Graffiti,
}
```
- **Purpose**: General metadata about validators
- **Optional Fields**: `index` may be `None` if validator not yet registered

### 2. Operator Module (`operator.rs`)

#### `OperatorId`
```rust
pub struct OperatorId(pub u64);
```
- **Purpose**: Unique 64-bit identifier for operators
- **Traits**: Full ordering, SSZ encoding, display formatting
- **Features**: Supports arbitrary fuzzing when feature enabled

#### `Operator`
```rust
pub struct Operator {
    pub id: OperatorId,
    pub rsa_pubkey: Rsa<Public>,
    pub owner: Address,
}
```
- **Purpose**: Network client maintaining system health
- **Key Methods**:
  - `new(pem_data: &[u8], operator_id: OperatorId, owner: Address) -> Result<Self, String>`
  - `new_with_pubkey(rsa_pubkey: Rsa<Public>, id: OperatorId, owner: Address) -> Self`
- **Validation**: PEM data must be valid base64-encoded RSA public key

### 3. Committee Module (`committee.rs`)

#### `CommitteeId`
```rust
pub struct CommitteeId(pub [u8; 32]);
```
- **Purpose**: Deterministic identifier for operator committees
- **Generation**: SHA256 hash of sorted operator IDs (as 32-bit LE bytes)
- **Conversions**: 
  - `From<Vec<OperatorId>>`: Sorts input before hashing
  - `From<&[OperatorId]>`: Direct hashing of slice
  - `TryFrom<&[u8]>`: From raw bytes (exactly 32 bytes required)

#### `CommitteeInfo`
```rust
pub struct CommitteeInfo {
    pub committee_members: IndexSet<OperatorId>,
    pub validator_indices: Vec<ValidatorIndex>,
}
```
- **Purpose**: Links committee members to their assigned validators

### 4. Message Module (`message.rs`)

#### Size Constraints
- `MAX_SIGNATURES`: 13 (maximum operators per message)
- `RSA_SIGNATURE_SIZE`: 256 bytes
- `MAX_FULL_DATA_SIZE`: 4,194,532 bytes
- `MAX_ENCODED_CONSENSUS_MSG_SIZE`: Calculated based on QBFT parameters
- `MAX_ENCODED_PARTIAL_SIGNATURE_SIZE`: Calculated based on signature parameters

#### `MsgType`
```rust
#[repr(u64)]
pub enum MsgType {
    SSVConsensusMsgType = 0,
    SSVPartialSignatureMsgType = 1,
}
```
- **Purpose**: Distinguishes consensus from partial signature messages
- **Encoding**: Custom SSZ implementation with u64 discriminant
- **Validation**: Only values 0 and 1 are valid

#### `SSVMessage`
```rust
pub struct SSVMessage {
    msg_type: MsgType,
    msg_id: MessageId,     // Fixed 56 bytes
    data: Vec<u8>,         // Variable length
}
```
- **Purpose**: Base message structure for SSV communication
- **Validation Rules**:
  - Data cannot be empty
  - Size limits based on message type
  - Consensus messages: ≤ `MAX_ENCODED_CONSENSUS_MSG_SIZE`
  - Partial signature messages: ≤ `MAX_ENCODED_PARTIAL_SIGNATURE_SIZE`
- **Constructor**: `new()` method validates constraints

#### `SignedSSVMessage`
```rust
pub struct SignedSSVMessage {
    signatures: Vec<Vec<u8>>,      // Max 13, each 256 bytes
    operator_ids: Vec<OperatorId>, // Max 13, must match signatures
    ssv_message: SSVMessage,
    full_data: Vec<u8>,           // Max 4,194,532 bytes
}
```
- **Purpose**: Cryptographically signed SSV message
- **Validation Rules**:
  - Maximum 13 signatures and operator IDs
  - Each signature exactly 256 bytes (RSA signature size)
  - Operator IDs must be sorted in ascending order
  - No duplicate operator IDs
  - No zero operator IDs
  - Signature count must equal operator ID count
  - Full data within size limits
- **Key Methods**:
  - `aggregate()`: Merges multiple signed messages, maintains sorting
  - `validate()`: Comprehensive validation of all constraints

### 5. Consensus Module (`consensus.rs`)

#### `QbftMessageType`
```rust
pub enum QbftMessageType {
    Proposal = 0,
    Prepare = 1,
    Commit = 2,
    RoundChange = 3,
}
```
- **Purpose**: QBFT consensus protocol message phases
- **Encoding**: Custom SSZ with u64 representation

#### `QbftMessage`
```rust
pub struct QbftMessage {
    pub qbft_message_type: QbftMessageType,
    pub height: u64,
    pub round: u64,
    pub identifier: VariableList<u8, U56>,
    pub root: Hash256,
    pub data_round: u64,
    pub round_change_justification: Vec<SignedSSVMessage>,
    pub prepare_justification: Vec<SignedSSVMessage>,
}
```
- **Purpose**: QBFT consensus protocol message
- **Constraints**: Justification messages always without full_data
- **Display**: Custom formatting showing length of justification arrays

#### `ValidatorDuty`
```rust
pub struct ValidatorDuty {
    pub r#type: BeaconRole,
    pub pub_key: PublicKeyBytes,
    pub slot: Slot,
    pub validator_index: ValidatorIndex,
    pub committee_index: CommitteeIndex,
    pub committee_length: u64,
    pub committees_at_slot: u64,
    pub validator_committee_index: u64,
    pub validator_sync_committee_indices: VariableList<u64, U13>,
}
```
- **Purpose**: Complete duty assignment for validators
- **Tree Hash**: Implements Merkle tree hashing for consensus

#### `BeaconRole`
Constants defining validator roles:
- `BEACON_ROLE_ATTESTER = 0`
- `BEACON_ROLE_AGGREGATOR = 1`
- `BEACON_ROLE_PROPOSER = 2`
- `BEACON_ROLE_SYNC_COMMITTEE = 3`
- `BEACON_ROLE_SYNC_COMMITTEE_CONTRIBUTION = 4`
- `BEACON_ROLE_VALIDATOR_REGISTRATION = 5`
- `BEACON_ROLE_VOLUNTARY_EXIT = 6`
- `BEACON_ROLE_UNKNOWN = u64::MAX`

#### `BeaconVote`
```rust
pub struct BeaconVote {
    pub block_root: Hash256,
    pub source: Checkpoint,
    pub target: Checkpoint,
}
```
- **Purpose**: Attestation vote data
- **QbftData Implementation**: Provides hashing and validation

#### `DataVersion`
```rust
pub struct DataVersion(ForkName);
```
- **Purpose**: Wrapper for Ethereum fork versions
- **Encoding**: Custom mapping (Base=1, Altair=2, Bellatrix=3, Capella=4, Deneb=5, Electra=6, Fulu=7)

#### `ContributionWrapper<E: EthSpec>`
```rust
pub struct ContributionWrapper<E: EthSpec> {
    pub contribution: Contribution<E>,
}
```
- **Purpose**: Workaround for Go-SSV encoding incompatibility
- **Behavior**: Forces variable-length SSZ encoding for fixed-length contributions

### 6. Share Module (`share.rs`)

#### `Share`
```rust
pub struct Share {
    pub validator_pubkey: PublicKeyBytes,
    pub operator_id: OperatorId,
    pub cluster_id: ClusterId,
    pub share_pubkey: PublicKeyBytes,
    pub encrypted_private_key: [u8; ENCRYPTED_KEY_LENGTH],
}
```
- **Purpose**: One of N shares of a split validator key
- **Security**: Private key encrypted with AES (256 bytes)
- **Linking**: Associates operator and cluster with validator key share

#### Constants
- `ENCRYPTED_KEY_LENGTH = 256`: Size of encrypted private key shares

### 7. Supporting Modules

#### Message ID (`msgid.rs`)
- **`MessageId`**: Fixed 56-byte identifier for message tracking
- **SSZ Encoding**: Transparent behavior as byte array

#### Partial Signatures (`partial_sig.rs`)
- Contains partial signature aggregation logic
- Handles threshold signature schemes

#### Round (`round.rs`)
- **`Round`**: Consensus round numbering
- Supports round progression in QBFT protocol

#### SQL Conversions (`sql_conversions.rs`)
- Database serialization helpers
- Converts SSV types to/from SQL-compatible formats

## Validation Rules Summary

### Message Validation
1. SSV messages must have non-empty data
2. Data size must not exceed type-specific limits
3. Message IDs must be exactly 56 bytes

### Signed Message Validation
1. Maximum 13 signatures per message
2. Each signature exactly 256 bytes
3. Operator IDs sorted, unique, non-zero
4. Signature count equals operator count
5. Full data within 4MB limit

### Cluster Validation
1. At least one operator required
2. Byzantine fault tolerance: minimum 3f+1 operators for f faults
3. Unique operator IDs within cluster

### Committee Validation
1. Deterministic ID generation from sorted operators
2. Consistent ordering across all operations

This specification ensures type safety, Byzantine fault tolerance, and interoperability within the SSV ecosystem.