# Message Validator Component Specification

## Component Interface

### Primary Types

#### `Validator<S: SlotClock, D: DutiesProvider>`
Main validation orchestrator with generic parameters for slot clock and duties provider.

**Constructor:**
```rust
pub fn new(
    network_state_rx: Receiver<NetworkState>,
    slots_per_epoch: u64,
    epochs_per_sync_committee_period: u64,
    sync_committee_size: usize,
    duties_provider: Arc<D>,
    slot_clock: S,
    task_executor: &TaskExecutor,
) -> Arc<Self>
```

**Primary Method:**
```rust
pub fn validate(&self, message_data: &[u8]) -> ValidationResult
```

#### `ValidationResult`
Enum representing validation outcomes:
```rust
pub enum ValidationResult {
    Success(ValidatedMessage),
    PreDecodeFailure(ValidationFailure),
    PostDecodeFailure(ValidationFailure, SignedSSVMessage),
}
```

#### `ValidationFailure`
Comprehensive enum of 46 different failure types covering all validation scenarios.

### Validation Pipeline

#### Stage 1: Message Decoding
- **Input**: Raw bytes from network layer
- **Process**: SSZ decoding to `SignedSSVMessage`
- **Output**: Decoded message or `PreDecodeFailure`

#### Stage 2: Message Type Routing
- **Consensus Messages**: Route to `validate_consensus_message()`
- **Partial Signatures**: Route to `validate_partial_signature_message()`

#### Stage 3: Validation Layers
1. **Semantic Validation**: Protocol rule enforcement
2. **QBFT Logic Validation**: Consensus-specific rules (for consensus messages)
3. **Duty Logic Validation**: Beacon chain duty alignment
4. **Cryptographic Validation**: RSA signature verification
5. **State Updates**: Update internal tracking state

## Consensus Message Validation Specification

### Message Types Supported
- **Proposal**: Leader-proposed values with optional justifications
- **Prepare**: Validator acceptance of proposals
- **Commit**: Validator commitment to values
- **RoundChange**: Round advancement requests

### Semantic Validation Rules

#### Multi-Signer Rules
- Only `Commit` messages may have multiple signers
- Multi-signer commits must meet quorum threshold: `(committee_size - 1) / 3 * 2 + 1`

#### Full Data Rules
- `Prepare` messages must not contain full data
- Single-signer `Commit` messages must not contain full data
- Multi-signer `Commit` messages must have full data matching root hash

#### Round Validation
- Round must be ≥ 1 (no zero rounds)
- Round must not exceed role-specific maximum
- Round must be within allowed spread from current time

#### Identifier Validation
- Message identifier must match SSV message identifier
- Role must support consensus (excludes `ValidatorRegistration`, `VoluntaryExit`)

#### Justification Rules
- Prepare justifications only allowed in `Proposal` messages
- Round change justifications only allowed in `Proposal` and `RoundChange` messages

### QBFT Logic Validation

#### Leader Validation
- `Proposal` messages must be signed by round leader
- Leader determined by round-robin: `(height + round - 1) % committee_size`

#### Round Advancement Rules
- Signers cannot decrease their round for same duty
- Messages in current round must pass message count limits
- Round must be within allowed spread: `[1, estimated_round + 3]`

#### State Consistency
- Multi-signer messages cannot reuse previous signer combinations
- Proposal data must be consistent across rounds

### Duty-Based Validation

#### Slot Advancement Rules
- For non-committee roles: operators cannot process earlier slots than their maximum
- Message slot must align with beacon chain timing constraints

#### Timing Validation
- **Earliness Limit**: 50ms clock error tolerance
- **Lateness Limits**:
  - Proposer/SyncCommittee: 1 + 2 slots + 3s margin
  - Committee/Aggregator: slots_per_epoch + 2 slots + 3s margin

#### Duty Assignment Validation
- **Proposer**: Must be assigned to propose for message slot
- **SyncCommittee**: Must be in sync committee for current period
- **Aggregator**: Duty assignment verified through duties provider
- **Committee**: No specific duty assignment required

#### Duty Count Limits
- **ValidatorRegistration/Aggregator**: 2 per epoch
- **VoluntaryExit**: Provider-determined limit
- **Committee**: `min(slots_per_epoch, 2 * validator_count)` with sync committee bonus

## Partial Signature Validation Specification

### Signature Types by Role
- **Committee**: `PostConsensus`
- **Aggregator**: `PostConsensus`, `SelectionProofPartialSig`
- **Proposer**: `PostConsensus`, `RandaoPartialSig`
- **SyncCommittee**: `PostConsensus`, `ContributionProofs`
- **ValidatorRegistration**: `ValidatorRegistration`
- **VoluntaryExit**: `VoluntaryExit`

### Semantic Validation Rules

#### Signer Requirements
- Must have exactly one signer (no multi-signer partial signatures)
- Message signer must match signature signer ID
- Must not contain full data

#### Validator Index Validation
- For non-committee roles: validator index must be in committee's validator set
- Committee role skips this validation (operators may not be synced on validator set)

### Message Count Limits

#### By Role
- **Committee**: `min(2 * validator_count, validator_count + sync_committee_size)`
- **SyncCommittee**: Maximum 13 signatures
- **All Others**: Maximum 1 signature per duty

#### Validator Index Constraints
- **Committee Role**: Each validator index may appear at most 2 times
- Triple validator index usage triggers `TripleValidatorIndexInPartialSignatures` error

### Duty-Based Validation
- Same slot advancement, timing, and duty assignment rules as consensus messages
- RANDAO messages have special timing tolerance during first slot of epoch

## State Management Specification

### `DutyState` Structure
```rust
pub struct DutyState {
    operators: HashMap<OperatorId, OperatorState>,
    stored_slot_count: usize,
}
```

#### Key Methods
- `get_or_create_operator()`: Lazy operator state initialization
- `update_for_consensus_message()`: Update state with consensus message
- `update_for_partial_signature()`: Update state with partial signature
- `outdated()`: Check if state can be garbage collected

### `OperatorState` Structure
Circular buffer-based state tracking with:
- `state: Vec<Option<SignerState>>`: Slot-indexed signer states
- `max_slot: Slot`: Highest processed slot
- `max_epoch: Epoch`: Highest processed epoch
- `curr_epoch_duties: u64`: Current epoch duty count
- `prev_epoch_duties: u64`: Previous epoch duty count

#### Key Methods
- `get_signer_state()`: Retrieve state for specific slot
- `is_first_message_for_duty()`: Check if first message for slot
- `get_duty_count()`: Get duty count for epoch

### `SignerState` Structure
Per-slot state tracking:
```rust
pub struct SignerState {
    slot: Slot,
    round: u64,
    message_counts: MessageCounts,
    proposal_hash: Option<[u8; 32]>,
    seen_signers: HashSet<CommitteeId>,
}
```

### `MessageCounts` Structure
Rate limiting enforcement:
```rust
pub struct MessageCounts {
    pre_consensus: u8,
    proposal: u8,
    prepare: u8,
    commit: u8,
    round_change: u8,
    post_consensus: u8,
}
```

#### Limits
- All message types: 1 per round maximum
- Multi-signer commits not counted (decided messages)

## Error Classification

### Critical Errors (Reject)
Messages that indicate malicious behavior or serious protocol violations:
- `SignatureVerification`
- `InvalidHash`
- `DecidedWithSameSigners`
- `DuplicatedMessage`
- `ZeroRound`

### Timing Errors (Ignore)
Messages that may be valid but mistimed:
- `EarlySlotMessage`
- `LateSlotMessage`
- `SlotAlreadyAdvanced`
- `RoundAlreadyAdvanced`

### Configuration Errors (Ignore)
Messages for unknown or misconfigured entities:
- `UnknownValidator`
- `ValidatorLiquidated`
- `NonExistentCommitteeID`
- `ValidatorNotAttesting`

### Protocol Errors (Reject)
Messages violating SSV protocol rules:
- `UnexpectedConsensusMessage`
- `PartialSigOneSigner`
- `PrepareOrCommitWithFullData`
- `RoundTooHigh`

## Performance Characteristics

### Memory Usage
- Circular buffers limit memory to `stored_slot_count * operators_count * slot_state_size`
- Default storage: 2 epochs worth of slots
- Automatic cleanup of outdated state

### Computational Complexity
- **RSA Signature Verification**: O(1) per signature, most expensive operation
- **State Lookups**: O(1) using hash maps and circular buffer indexing
- **Validation Logic**: O(1) for most rules, O(n) for some multi-signature validations

### Concurrency
- Thread-safe using `DashMap` for concurrent operator access
- Lock-free reads for most validation operations
- Write locks only during state updates

## Configuration Parameters

### Timing Constants
- `CLOCK_ERROR_TOLERANCE`: 50ms
- `LATE_MESSAGE_MARGIN`: 3s
- `LATE_SLOT_ALLOWANCE`: 2 slots

### Round Management
- `FIRST_ROUND`: 1
- `MAX_ALLOWED_ROUNDS_FUTURE`: 3
- `QUICK_TIMEOUT_THRESHOLD`: 8 rounds
- `QUICK_TIMEOUT`: 2s
- `SLOW_TIMEOUT`: 120s

### Message Limits
- `MAX_MESSAGES_PER_ROUND`: 1
- `MAX_SIGNATURES_IN_SYNC_COMMITTEE`: 13

### Storage
- Default `stored_slot_count`: 2 epochs
- Cleanup frequency: Once per epoch at 5/6 through first slot