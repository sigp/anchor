# Anchor Validator Store - Component Specification

## Module Structure

### Core Module (`lib.rs`)

#### AnchorValidatorStore<T: SlotClock, E: EthSpec>

**Fields:**
- `validators: DashMap<PublicKeyBytes, InitializedValidator>` - Active validator registry
- `validators_per_committee: DashMap<CommitteeId, HashSet<ValidatorIndex>>` - Committee membership mapping
- `signature_collector: Arc<SignatureCollectorManager>` - Distributed signature coordination
- `qbft_manager: Arc<QbftManager>` - Byzantine fault tolerant consensus
- `slashing_protection: SlashingDatabase` - Validator safety enforcement
- `slot_clock: T` - Network time synchronization
- `spec: Arc<ChainSpec>` - Ethereum consensus specification
- `private_key: Option<Rsa<Private>>` - Key share decryption key

**Constructor Parameters:**
```rust
pub fn new(
    database_state: watch::Receiver<NetworkState>,
    signature_collector: Arc<SignatureCollectorManager>,
    qbft_manager: Arc<QbftManager>,
    slashing_protection: SlashingDatabase,
    disable_slashing_protection: bool,
    slot_clock: T,
    spec: Arc<ChainSpec>,
    genesis_validators_root: Hash256,
    private_key: Option<Rsa<Private>>,
    task_executor: TaskExecutor,
    gas_limit: u64,
    builder_proposals: bool,
    builder_boost_factor: Option<u64>,
    prefer_builder_proposals: bool,
    is_synced: watch::Receiver<bool>,
) -> Arc<AnchorValidatorStore<T, E>>
```

#### Core Methods

**Validator Management:**
- `load_validators(&self, state: &NetworkState)` - Sync validators from database state
- `add_validator()` - Register new validator with slashing protection
- `remove_validator()` - Deregister validator and cleanup committee membership
- `get_share_from_state()` - Decrypt validator key share using RSA private key

**Consensus Operations:**
- `decide_abstract_block()` - QBFT consensus for block proposals
- `sign_abstract_block()` - Sign consensus-agreed block with slashing checks
- `collect_signature()` - Coordinate distributed signature collection

**Utility Methods:**
- `get_domain()` - Calculate signing domain for given epoch/domain type
- `timeout_within_slot()` - Execute future with slot-based timeout
- `get_instant_in_slot()` - Calculate target instant within slot timing

#### Data Structures

**InitializedValidator:**
```rust
struct InitializedValidator {
    cluster: Cluster,                    // SSV cluster configuration
    metadata: ValidatorMetadata,         // Validator registration data
    decrypted_key_share: Option<SecretKey>, // BLS key share for signing
}
```

**SlotMetadata<E: EthSpec>:**
```rust
struct SlotMetadata<E: EthSpec> {
    slot: Slot,                                                    // Target slot
    beacon_vote: BeaconVote,                                      // Consensus beacon vote
    attesting_validators: Vec<ValidatorIndex>,                    // Attestation duties
    sync_validators: Vec<ValidatorIndex>,                         // Sync committee duties
    multi_sync_aggregators: HashMap<PublicKeyBytes, ContributionWaiter<E>>, // Multi-subnet sync
}
```

**ContributionWaiter<E: EthSpec>:**
```rust
struct ContributionWaiter<E: EthSpec> {
    data: RwLock<Vec<ContributionAndProofSigningData<E>>>, // Contribution collection
    barrier: Barrier,                                      // Synchronization barrier
}
```

### Metadata Service Module (`metadata_service.rs`)

#### MetadataService<E: EthSpec, T: SlotClock + 'static>

**Fields:**
- `duties_service: Arc<DutiesService<AnchorValidatorStore<T, E>, T>>` - Duty assignment service
- `validator_store: Arc<AnchorValidatorStore<T, E>>` - Validator store reference
- `slot_clock: T` - Network time synchronization
- `beacon_nodes: Arc<BeaconNodeFallback<T>>` - Beacon node connectivity
- `executor: TaskExecutor` - Background task management
- `spec: Arc<ChainSpec>` - Consensus specification

**Key Methods:**
- `start_update_service()` - Initialize metadata update background service
- `update_metadata()` - Fetch and update slot metadata from beacon nodes

### Metrics Module (`metrics.rs`)

**Consensus Timing Metrics:**
- `CONSENSUS_TIMES` - Duration histograms for consensus operations
- `SIGNED_RANDAO_REVEALS_TOTAL` - Counter for RANDAO reveal signings

**Metric Labels:**
- `AGGREGATE_AND_PROOF` - Aggregate and proof consensus timing
- `BLOCK` - Block proposal consensus timing  
- `BEACON_VOTE` - Beacon vote consensus timing
- `SYNC_CONTRIBUTION_AND_PROOF` - Sync contribution consensus timing

## ValidatorStore Trait Implementation

### Required Methods

**Validator Information:**
- `validator_index(&self, pubkey: &PublicKeyBytes) -> Option<u64>`
- `voting_pubkeys<I, F>(&self, filter_func: F) -> I`
- `num_voting_validators(&self) -> usize`
- `graffiti(&self, validator_pubkey: &PublicKeyBytes) -> Option<Graffiti>`
- `get_fee_recipient(&self, validator_pubkey: &PublicKeyBytes) -> Option<Address>`

**Builder Configuration:**
- `determine_builder_boost_factor(&self, validator_pubkey: &PublicKeyBytes) -> Option<u64>`

**Signing Operations:**
- `randao_reveal(&self, validator_pubkey: PublicKeyBytes, signing_epoch: Epoch) -> Result<Signature, Error>`
- `sign_block(&self, validator_pubkey: PublicKeyBytes, block: UnsignedBlock<E>, current_slot: Slot) -> Result<SignedBlock<E>, Error>`
- `sign_attestation(&self, validator_pubkey: PublicKeyBytes, validator_committee_position: usize, attestation: &mut Attestation<E>, current_epoch: Epoch) -> Result<(), Error>`
- `sign_validator_registration_data(&self, validator_registration_data: ValidatorRegistrationData) -> Result<SignedValidatorRegistrationData, Error>`

**Aggregation Operations:**
- `produce_signed_aggregate_and_proof(&self, ...) -> Result<SignedAggregateAndProof<E>, Error>`
- `produce_selection_proof(&self, validator_pubkey: PublicKeyBytes, slot: Slot) -> Result<SelectionProof, Error>`

**Sync Committee Operations:**
- `produce_sync_selection_proof(&self, validator_pubkey: &PublicKeyBytes, slot: Slot, subnet_id: SyncSubnetId) -> Result<SyncSelectionProof, Error>`
- `produce_sync_committee_signature(&self, slot: Slot, beacon_block_root: Hash256, validator_index: u64, validator_pubkey: &PublicKeyBytes) -> Result<SyncCommitteeMessage, Error>`
- `produce_signed_contribution_and_proof(&self, ...) -> Result<SignedContributionAndProof<E>, Error>`

## Error Handling

### SpecificError Variants
- `SignatureCollectionFailed(CollectionError)` - Distributed signature collection failure
- `QbftError(QbftError)` - QBFT consensus failure
- `Timeout` - Operation exceeded slot timing constraints
- `InvalidQbftData(DecodeError)` - Malformed consensus data
- `TooManySyncSubnetsToSign` - Excessive sync committee subnet assignments
- `MissingIndex` - Validator index not available
- `NotSynced` - Beacon node synchronization required

### Slashing Protection Integration
- Automatic safety checks for all signing operations
- Historical data pruning with configurable retention
- Support for testing environments with disabled protection

## Configuration Constants

- `SLASHING_PROTECTION_HISTORY_EPOCHS: u64 = 512` - Slashing protection retention period
- Various log name constants for metric tracking and error reporting

## Thread Safety and Concurrency

- Uses `DashMap` for concurrent validator registry access
- Atomic operations for validator committee membership
- Background service coordination via `TaskExecutor`
- Watch channels for real-time state synchronization