# Duties Tracker Component Specification

## API Reference

### DutiesProvider Trait

The main interface for querying validator duties.

```rust
pub trait DutiesProvider: Sync + Send + 'static {
    fn is_validator_in_sync_committee(&self, committee_period: u64, validator_index: ValidatorIndex) -> bool;
    fn is_epoch_known_for_proposers(&self, epoch: Epoch) -> bool;
    fn is_validator_proposer_at_slot(&self, slot: Slot, validator_index: ValidatorIndex) -> bool;
    fn get_voluntary_exit_duty_count(&self, slot: Slot, pubkey: &PublicKeyBytes) -> u64;
}
```

### DutiesTracker

**Constructor**: `duties_tracker.rs:48-65`
```rust
pub fn new(
    voluntary_exit_tracker: Arc<VoluntaryExitTracker>,
    beacon_nodes: Arc<BeaconNodeFallback<T>>,
    spec: Arc<ChainSpec>,
    slots_per_epoch: u64,
    slot_clock: T,
    network_state_rx: watch::Receiver<NetworkState>,
) -> Self
```

**Lifecycle**: `duties_tracker.rs:244-265`
```rust
pub fn start(self: Arc<Self>, executor: TaskExecutor)
```

### Data Structures

#### Duties
**Location**: `lib.rs:72-87`

```rust
pub struct Duties {
    pub proposers: RwLock<HashMap<Epoch, Vec<ProposerData>>>,
    pub sync_duties: SyncCommitteePerPeriod,
}
```

#### SyncCommitteePerPeriod  
**Location**: `lib.rs:28-67`

```rust
pub struct SyncCommitteePerPeriod {
    committees: DashMap<u64, HashSet<u64>>,
}
```

**Methods**:
- `all_duties_known(&self, committee_period: u64, validator_indices: &[u64]) -> bool`
- `prune(&self, current_sync_committee_period: u64)`
- `is_validator_in_sync_committee(&self, committee_period: u64, validator_index: u64) -> bool`

#### VoluntaryExitTracker
**Location**: `voluntary_exit_tracker.rs:17-117`

```rust
pub struct VoluntaryExitTracker {
    scheduled_exits: DashMap<Slot, Vec<ExitDuty>>,
    all_duties_by_slot: DashMap<Slot, HashMap<PublicKeyBytes, u64>>,
}
```

**Methods**:
- `add_duty_for_slot(&self, slot: Slot, pubkey: PublicKeyBytes, validator_index: ValidatorIndex, is_own_validator: bool) -> bool`
- `get_ready_exits(&self, current_slot: Slot) -> Vec<ExitDuty>`
- `remove_processed_exit(&self, exit_duty: &ExitDuty)`
- `get_duty_count(&self, slot: Slot, pubkey: &PublicKeyBytes) -> u64`
- `prune(&self, current_slot: Slot, lookback: u64)`

## Constants

- `HISTORICAL_DUTIES_EPOCHS`: `2` - Number of historical epochs to retain
- Sync committee duty offset: `epochs_per_sync_committee_period / 2`

## Error Types

```rust
#[derive(Error, Debug)]
pub enum Error {
    #[error("Unable to read the slot clock")]
    UnableToReadSlotClock,
    #[error("Arithmetic error")]
    Arith(ArithError),
    #[error("Failed to poll proposers: {0}")]
    FailedToPollProposers(String),
}
```

## Polling Behavior

### Sync Committee Duties
**Function**: `duties_tracker.rs:67-122`

**Polling Logic**:
1. Check if Altair fork is activated
2. Calculate current and next sync committee periods
3. Poll current period duties if unknown
4. Poll next period duties if past epoch offset threshold
5. Prune old duties after successful polls

**Timing**: Runs every slot via `spawn_polling_task`

### Proposer Duties  
**Function**: `duties_tracker.rs:191-242`

**Polling Logic**:
1. Poll current epoch proposer duties
2. Filter to only include local validators
3. Store in proposers map
4. Prune duties older than `HISTORICAL_DUTIES_EPOCHS`

**Timing**: Runs every slot via `spawn_polling_task`

## Memory Management

### Pruning Strategies

1. **Sync Committee Duties**: Retain only current sync committee period and later
2. **Proposer Duties**: Retain current epoch + `HISTORICAL_DUTIES_EPOCHS` (2 epochs)
3. **Voluntary Exit Duties**: Configurable lookback period

### Storage Optimization

- **Sparse Storage**: Only store validators with actual duties
- **Fine-grained Locking**: DashMap provides per-entry locking
- **Efficient Lookups**: HashMap-based storage for O(1) access patterns

## Thread Safety Guarantees

### Concurrent Data Structures
- `DashMap<K, V>`: Concurrent HashMap with fine-grained locking
- `RwLock<T>`: Multiple readers, single writer access
- `Arc<T>`: Thread-safe reference counting

### Async Safety
- All polling operations are async-safe
- No blocking operations in async contexts
- Proper cancellation handling via TaskExecutor

## Integration Requirements

### Required Dependencies
- `beacon_node_fallback`: For beacon node API access
- `slot_clock`: For time synchronization  
- `database::NetworkState`: For validator state
- `task_executor`: For async task management

### Network Requirements
- Beacon node API access for duty polling
- Reliable network connectivity for continuous updates

## Performance Characteristics

### Time Complexity
- Duty lookups: O(1) average case
- Sync committee checks: O(1) 
- Proposer duty checks: O(n) where n = validators per epoch

### Space Complexity
- Sync duties: O(validators_with_duties × periods)
- Proposer duties: O(validators × retained_epochs)
- Exit duties: O(scheduled_exits)

### Concurrency
- Read-heavy workloads scale with number of CPU cores
- Write operations (duty updates) are serialized per data structure
- Multiple data structures can be updated concurrently