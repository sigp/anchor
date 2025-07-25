# Subnet Service Component Specification

## Module Structure

```
subnet_service/
├── src/
│   ├── lib.rs              # Main subnet service implementation
│   └── message_rate.rs     # Message rate calculation algorithms
└── Cargo.toml
```

## Type Definitions

### SubnetId

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SubnetId(#[serde(with = "serde_utils::quoted_u64")] u64);
```

**Purpose**: Type-safe wrapper for subnet identifiers

**Key Methods**:
- `new(id: u64) -> Self`: Create from raw u64
- `from_committee(committee_id: CommitteeId, subnet_count: usize) -> Self`: Derive from committee ID
- Implements `Deref<Target = u64>` for transparent u64 access

**Derivation Algorithm**:
```rust
let id = U256::from_be_bytes(*committee_id);
SubnetId((id % U256::from(subnet_count)).try_into().expect("modulo must be < subnet_count"))
```

### SubnetEvent

```rust
pub enum SubnetEvent {
    Join(SubnetId, Option<f64>),    // subnet_id and optional message_rate
    Leave(SubnetId),
    RateUpdate(SubnetId, f64),      // subnet_id and new message_rate
}
```

**Event Types**:
- `Join`: Subscribe to subnet with optional message rate for scoring
- `Leave`: Unsubscribe from subnet
- `RateUpdate`: Update message rate for existing subscription (scoring only)

### MessageCounts

```rust
#[derive(Debug, Clone, Copy)]
pub struct MessageCounts {
    pub pre_consensus: usize,
    pub consensus: usize,
    pub post_consensus: usize,
}
```

**Message Type Formulas**:
- **Consensus**: `1 + committee_size + committee_size + 2` (proposal + prepares + commits + decided)
- **Partial Signature**: `committee_size`
- **With Pre-consensus**: `committee_size + consensus + committee_size`
- **Without Pre-consensus**: `0 + consensus + committee_size`

## Core Functions

### start_subnet_service

```rust
pub fn start_subnet_service<E: EthSpec>(
    db: watch::Receiver<NetworkState>,
    subnet_count: usize,
    subscribe_all_subnets: bool,
    disable_gossipsub_topic_scoring: bool,
    executor: &TaskExecutor,
    slot_clock: impl SlotClock + 'static,
    chain_spec: Arc<ChainSpec>,
) -> mpsc::Receiver<SubnetEvent>
```

**Parameters**:
- `db`: Watch receiver monitoring network state changes
- `subnet_count`: Total subnets in network (typically 128)
- `subscribe_all_subnets`: Initial subscription mode
- `disable_gossipsub_topic_scoring`: Skip message rate calculations
- `executor`: Task executor for background service
- `slot_clock`: Ethereum slot timing provider
- `chain_spec`: Chain-specific parameters

**Return**: Channel receiver for subnet events

**Behavior**:
1. Creates bounded channel with capacity based on subscription mode
2. Spawns background `subnet_service` task
3. Returns receiver end for event consumption

### subnet_service (Background Task)

**State Management**:
- Tracks `previous_subnets: HashSet<SubnetId>` for diff calculations
- Calculates `next_epoch_delay` for epoch boundary updates
- Monitors database changes via watch channel

**Main Loop**:
```rust
loop {
    tokio::select! {
        _ = db.changed(), if !subscribe_all_subnets => {
            handle_subnet_changes().await;
        }
        _ = sleep(next_epoch_delay), if !disable_gossipsub_topic_scoring => {
            handle_epoch_committee_update().await;
            next_epoch_delay = calculate_duration_to_next_epoch();
        }
    }
}
```

## Algorithm Specifications

### Subnet Assignment Algorithm

**Input**: `committee_id: CommitteeId`, `subnet_count: usize`

**Process**:
1. Convert committee ID to U256: `U256::from_be_bytes(*committee_id)`
2. Apply modulo operation: `id % subnet_count`
3. Convert back to u64 with bounds checking

**Properties**:
- Deterministic mapping from committees to subnets
- Uniform distribution across subnets
- Collision-resistant for different committee IDs

### Committee Change Detection

**Input**: Current network state, previous subnet set

**Process**:
1. Build current subnet set from owned clusters:
   ```rust
   for cluster_id in state.get_own_clusters() {
       if let Some(cluster) = state.clusters().get_by(cluster_id) {
           let subnet_id = SubnetId::from_committee(cluster.committee_id(), subnet_count);
           current_subnets.insert(subnet_id);
       }
   }
   ```

2. Calculate set differences:
   - `to_leave = previous_subnets - current_subnets`
   - `to_join = current_subnets - previous_subnets`

3. Emit appropriate events for changes

### Message Rate Calculation

**Core Formula** (from `message_rate.rs`):
```rust
pub fn calculate_message_rate_for_topic<E: EthSpec>(
    committees: &[CommitteeInfo],
    chain_spec: &ChainSpec,
) -> f64
```

**Components**:

1. **Attestation Duties**: `expected_committee_duties_per_epoch_due_to_attestation() * duties_without_pre_consensus`
2. **Sync Committee Duties**: `expected_single_sc_committee_duties_per_epoch() * duties_without_pre_consensus`  
3. **Aggregator Duties**: `num_validators * aggregator_probability() * duties_with_pre_consensus`
4. **Proposal Duties**: `num_validators * slots_per_epoch * PROPOSAL_PROBABILITY * duties_with_pre_consensus`
5. **Sync Aggregation**: `num_validators * slots_per_epoch * sync_committee_agg_prob * duties_with_pre_consensus`

**Final Conversion**: `total_msg_rate / (slots_per_epoch * slot_duration_seconds)`

### Duty Probability Models

**Attestation Committee Duties**:
```rust
fn expected_committee_duties_per_epoch_due_to_attestation<E: EthSpec>(num_validators: usize) -> f64 {
    let k = num_validators as f64;
    let n = E::slots_per_epoch() as f64;
    let probability_all_not_on_slot_i = ((n - 1.0) / n).powf(k);
    let probability_at_least_one_on_slot_i = 1.0 - probability_all_not_on_slot_i;
    n * probability_at_least_one_on_slot_i
}
```

**Sync Committee Duties**:
```rust
fn expected_single_sc_committee_duties_per_epoch<E: EthSpec>(num_validators: usize) -> f64 {
    let sync_committee_probability = sync_committee_size / ETHEREUM_VALIDATORS;
    let chance_of_not_being_in_sync_committee = 1.0 - sync_committee_probability;
    let chance_that_all_validators_are_not_in_sync_committee = 
        chance_of_not_being_in_sync_committee.powf(num_validators as f64);
    let chance_of_at_least_one_validator_being_in_sync_committee = 
        1.0 - chance_that_all_validators_are_not_in_sync_committee;
    let expected_slots_with_no_duty = slots_per_epoch - expected_attestation_duties;
    chance_of_at_least_one_validator_being_in_sync_committee * expected_slots_with_no_duty
}
```

**Constants**:
- `ETHEREUM_VALIDATORS: f64 = 1_000_000.0`
- `PROPOSAL_PROBABILITY: f64 = 1.0 / ETHEREUM_VALIDATORS`
- `MAX_VALIDATORS_PER_COMMITTEE_LIST_CUT: usize = 560`
- `SINGLE_SC_DUTIES_LIMIT: f64 = 0.0`

### Epoch Boundary Timing

```rust
fn calculate_duration_to_next_epoch<E: EthSpec>(slot_clock: &impl SlotClock) -> Duration {
    if let Some(duration_to_next_epoch) = slot_clock.duration_to_next_epoch(E::slots_per_epoch()) {
        duration_to_next_epoch
    } else {
        let slot_duration = slot_clock.slot_duration();
        slot_duration * 3  // Fallback: wait 3 slots
    }
}
```

## Configuration Parameters

### Constants

- `SUBNET_COUNT: usize = 128`: Default number of subnets
- `SubnetBits: [u8; SUBNET_COUNT / 8]`: Bitmap type for subnet representation

### Runtime Configuration

- **Channel Capacity**: `subnet_count` for all-subnets mode, `1` for dynamic mode
- **Epoch Timing**: Based on `EthSpec::slots_per_epoch()` and `ChainSpec::seconds_per_slot`
- **Message Rate Calculation**: Configurable via `disable_gossipsub_topic_scoring`

## Error Handling Specifications

### Channel Errors
- **Send Failures**: Log warning and exit task (receiver dropped)
- **Watch Channel**: Handle database disconnection gracefully

### Timing Errors
- **Slot Clock Failures**: Fall back to conservative 3-slot intervals
- **Epoch Calculation**: Use fallback timing when current slot unavailable

### Data Validation
- **Committee ID Bounds**: Panic on modulo overflow (should never happen)
- **Subnet Count Validation**: Implicit through type system and constants

## Performance Characteristics

### Time Complexity
- **Subnet Assignment**: O(1) per committee
- **Change Detection**: O(n) where n = number of owned clusters
- **Message Rate Calculation**: O(c * v) where c = committees, v = validators per committee

### Space Complexity
- **State Storage**: O(s) where s = number of active subnets
- **Event Buffer**: Bounded by channel capacity configuration

### Concurrency
- **Database Access**: Non-blocking watch channel reads
- **Event Emission**: Async channel sends with backpressure
- **Timing**: Non-blocking sleep futures for epoch boundaries

## Testing Specifications

### Unit Tests (message_rate.rs)
- Message count calculations for different committee sizes
- Duty probability calculations with edge cases
- Mathematical properties of duty functions
- Rate calculation scaling and additivity
- Committee configuration variations

### Integration Points
- Database state change handling
- Channel communication patterns  
- Epoch boundary timing accuracy
- Committee-to-subnet mapping consistency

### Test Utilities
- `test_tracker()`: Mock subnet service for testing consumers
- Committee info creation helpers
- Configurable test chain specifications