# QBFT Component Specification

## Component Identity
- **Name**: QBFT (Quorum Based Fault Tolerance)
- **Path**: `anchor/common/qbft/`
- **Type**: Consensus Algorithm Implementation
- **Language**: Rust

## Purpose
Implements the QBFT consensus protocol for Byzantine fault-tolerant agreement in distributed systems. This component is designed for the Anchor SSV (Secret Shared Validator) network to achieve consensus on validator duties and operations.

## Public API

### Main Types

#### `Qbft<F, D, S>`
The core consensus instance managing the entire QBFT process.

**Generics:**
- `F: LeaderFunction + Clone` - Leader selection algorithm
- `D: QbftData<Hash = Hash256>` - Consensus data type  
- `S: MessageSender` - Message output interface

**Key Methods:**
```rust
pub fn new(config: Config<F>, start_data: D, identifier: MessageId, message_sender: S) -> Self
pub fn receive(&mut self, wrapped_msg: WrappedQbftMessage)
pub fn end_round(&mut self)
pub fn completed(&self) -> Option<Completed<D>>
pub fn get_aggregated_commit(&self) -> Option<SignedSSVMessage>
```

#### `Config<F>`
Configuration for QBFT instances with builder pattern support.

**Key Fields:**
- `operator_id: OperatorId` - This node's identifier
- `committee_members: IndexSet<OperatorId>` - Committee participants
- `quorum_size: usize` - Messages needed for consensus
- `round_time: Duration` - Round timeout duration
- `max_rounds: usize` - Maximum rounds before giving up

#### `ConfigBuilder<F>`
Builder for creating validated QBFT configurations.

**Usage:**
```rust
let config = ConfigBuilder::new(operator_id, height, committee)
    .with_round_time(Duration::from_secs(5))
    .with_max_rounds(10)
    .build()?;
```

### Traits

#### `LeaderFunction`
Defines leader selection logic for each round.

```rust
pub trait LeaderFunction {
    fn leader_function(
        &self,
        operator_id: &OperatorId,
        round: Round,
        instance_height: InstanceHeight,
        committee: &IndexSet<OperatorId>,
    ) -> bool;
}
```

#### `MessageSender`
Interface for sending consensus messages.

```rust
pub trait MessageSender {
    fn send(&mut self, msg: UnsignedWrappedQbftMessage);
}
```

### Message Types

#### `WrappedQbftMessage`
Incoming signed consensus message with validation.

#### `UnsignedWrappedQbftMessage`  
Outgoing unsigned message ready for signing.

#### `InstanceState`
Current state of consensus instance:
- `AwaitingProposal` - Waiting for leader proposal
- `Prepare { proposal_root }` - Validating proposal
- `Commit { proposal_root }` - Committing to value
- `SentRoundChange` - Initiated round change
- `Complete` - Consensus finished

#### `Completed<D>`
Final result of consensus:
- `Success(D)` - Agreed on data value
- `TimedOut` - Failed to reach consensus

## Dependencies

### Internal
- `ssv_types` - SSV message and consensus types
- `types::Hash256` - Hash type for data identification

### External  
- `ssz` - Serialization for network messages
- `tracing` - Structured logging
- `indexmap` - Ordered sets for committees
- `derive_more` - Derive utilities

## Configuration

### Fault Tolerance
- Supports up to `f = (n-1)/3` Byzantine failures
- Requires `2f+1` signatures for consensus quorum
- Default quorum size: `committee_size - f`

### Timing
- Default round time: 2 seconds
- Default max rounds: 4
- Configurable per instance

### Validation Rules
- Committee size must be > 0
- Operator must be committee member  
- Quorum size: `2f+1 ≤ quorum ≤ n-f`
- Max rounds must be > 0
- Starting round ≤ max rounds

## Error Handling

### Configuration Errors (`ConfigBuilderError`)
- `NoParticipants` - Empty committee
- `OperatorNotParticipant` - Operator not in committee
- `InvalidQuorumSize` - Quorum outside valid range
- `ZeroMaxRounds` - No rounds configured
- `ExceedingStartingRound` - Invalid starting round

### Runtime Behavior
- Invalid messages logged and ignored
- Duplicate messages detected and discarded
- State transitions validated before execution
- Graceful handling of network delays and reordering

## Thread Safety
- Not thread-safe by design (single-threaded consensus)
- External synchronization required for concurrent access
- Message sender interface allows async integration

## Performance Characteristics
- O(1) message validation
- O(n) space complexity for message storage
- O(n²) worst-case for justification validation
- Memory usage grows with committee size and round count

## Integration Points

### Network Layer
- Receives `WrappedQbftMessage` from network
- Sends `UnsignedWrappedQbftMessage` via callback
- Message signing handled externally

### Data Layer  
- Generic over consensus data type `D`
- Data validation via `QbftData::validate()`
- SSZ serialization for network transmission

### Timing Layer
- Round timeouts handled externally via `end_round()`
- No internal timers or async operations
- Duration configuration for timeout hints

## Testing Strategy
- Unit tests for individual components
- Integration tests for full consensus flows
- Byzantine behavior simulation
- Network partition scenarios
- Message ordering and timing edge cases