# Duties Tracker Usage Examples

## Basic Setup

### Creating a DutiesTracker Instance

```rust
use std::sync::Arc;
use duties_tracker::{DutiesTracker, voluntary_exit_tracker::VoluntaryExitTracker};
use beacon_node_fallback::BeaconNodeFallback;
use slot_clock::SystemTimeSlotClock;
use tokio::sync::watch;
use database::NetworkState;

// Initialize dependencies
let voluntary_exit_tracker = Arc::new(VoluntaryExitTracker::new());
let beacon_nodes = Arc::new(beacon_node_fallback); // Assume this is configured
let spec = Arc::new(chain_spec); // Chain specification
let slots_per_epoch = 32;
let slot_clock = SystemTimeSlotClock::new(/* config */);
let (network_state_tx, network_state_rx) = watch::channel(NetworkState::default());

// Create duties tracker
let duties_tracker = Arc::new(DutiesTracker::new(
    voluntary_exit_tracker,
    beacon_nodes,
    spec,
    slots_per_epoch,
    slot_clock,
    network_state_rx,
));
```

### Starting the Duties Tracker

```rust
use task_executor::TaskExecutor;

let executor = TaskExecutor::new();
duties_tracker.start(executor);

// The tracker will now automatically poll for duties every slot
```

## Using the DutiesProvider Interface

### Checking Sync Committee Membership

```rust
use duties_tracker::DutiesProvider;
use ssv_types::ValidatorIndex;

let duties_provider: Arc<dyn DutiesProvider> = duties_tracker.clone();

// Check if validator is in sync committee for period 100
let validator_index = ValidatorIndex::from(12345);
let committee_period = 100;

if duties_provider.is_validator_in_sync_committee(committee_period, validator_index) {
    println!("Validator {} is in sync committee for period {}", validator_index, committee_period);
}
```

### Checking Proposer Duties

```rust
use types::{Epoch, Slot};

// Check if proposer duties are known for an epoch
let epoch = Epoch::new(1000);
if duties_provider.is_epoch_known_for_proposers(epoch) {
    println!("Proposer duties are known for epoch {}", epoch);
    
    // Check if specific validator is proposer at a slot
    let slot = Slot::new(32000); // slot in the epoch
    let validator_index = ValidatorIndex::from(54321);
    
    if duties_provider.is_validator_proposer_at_slot(slot, validator_index) {
        println!("Validator {} should propose at slot {}", validator_index, slot);
    }
}
```

### Checking Voluntary Exit Duties

```rust
use bls::PublicKeyBytes;

let slot = Slot::new(32000);
let pubkey = PublicKeyBytes::from([0u8; 48]); // Example pubkey

let exit_duty_count = duties_provider.get_voluntary_exit_duty_count(slot, &pubkey);
println!("Validator has {} exit duties at slot {}", exit_duty_count, slot);
```

## Working with VoluntaryExitTracker

### Adding Exit Duties

```rust
use duties_tracker::voluntary_exit_tracker::{VoluntaryExitTracker, ExitDuty};

let exit_tracker = Arc::new(VoluntaryExitTracker::new());

// Add an exit duty for our own validator
let slot = Slot::new(32000);
let pubkey = PublicKeyBytes::from([1u8; 48]);
let validator_index = ValidatorIndex::from(12345);
let is_own_validator = true;

let was_scheduled = exit_tracker.add_duty_for_slot(
    slot,
    pubkey,
    validator_index,
    is_own_validator,
);

if was_scheduled {
    println!("Exit duty scheduled for processing");
}
```

### Processing Ready Exits

```rust
// Get exits ready for processing at current slot
let current_slot = Slot::new(32000);
let ready_exits = exit_tracker.get_ready_exits(current_slot);

for exit_duty in ready_exits {
    println!("Processing exit for validator {} at slot {}", 
             exit_duty.validator_index, exit_duty.target_slot);
    
    // Process the exit...
    // After successful processing, remove it
    exit_tracker.remove_processed_exit(&exit_duty);
}
```

### Pruning Old Exit Data

```rust
// Prune exits older than 100 slots
let current_slot = Slot::new(32000);
let lookback_slots = 100;

exit_tracker.prune(current_slot, lookback_slots);
```

## Advanced Usage

### Custom Duty Polling Integration

```rust
use duties_tracker::{Duties, DutiesProvider};

// If you need to integrate with custom polling logic
async fn custom_duty_check<T: DutiesProvider>(
    duties_provider: &T,
    validators: &[ValidatorIndex],
    current_slot: Slot,
) {
    let current_epoch = current_slot.epoch(32); // 32 slots per epoch
    
    // Check if we need proposer duties
    if !duties_provider.is_epoch_known_for_proposers(current_epoch) {
        println!("Need to poll proposer duties for epoch {}", current_epoch);
    }
    
    // Check sync committee duties for each validator
    let committee_period = current_epoch.as_u64() / 256; // epochs per sync committee period
    for &validator_index in validators {
        if duties_provider.is_validator_in_sync_committee(committee_period, validator_index) {
            println!("Validator {} has sync duties in period {}", validator_index, committee_period);
        }
    }
}
```

### Monitoring Duty Updates

```rust
use tracing::{info, warn};

// Example of monitoring duty status
async fn monitor_duties<T: DutiesProvider>(duties_provider: &T) {
    let current_slot = slot_clock.now().unwrap();
    let current_epoch = current_slot.epoch(32);
    
    if duties_provider.is_epoch_known_for_proposers(current_epoch) {
        info!("Proposer duties up to date for epoch {}", current_epoch);
    } else {
        warn!("Missing proposer duties for epoch {}", current_epoch);
    }
}
```

### Error Handling Example

```rust
use duties_tracker::duties_tracker::Error;

async fn handle_duty_errors() -> Result<(), Error> {
    // Example error handling for duty operations
    match duties_tracker.poll_beacon_proposers().await {
        Ok(()) => {
            tracing::info!("Successfully polled proposer duties");
        }
        Err(Error::UnableToReadSlotClock) => {
            tracing::error!("Slot clock unavailable, retrying later");
            return Err(Error::UnableToReadSlotClock);
        }
        Err(Error::FailedToPollProposers(msg)) => {
            tracing::warn!("Failed to poll proposers: {}, will retry", msg);
            // Continue operation, will retry next slot
        }
        Err(Error::Arith(e)) => {
            tracing::error!("Arithmetic error in duty calculation: {:?}", e);
            return Err(Error::Arith(e));
        }
    }
    Ok(())
}
```

## Testing Examples

### Mock DutiesProvider for Tests

```rust
use duties_tracker::DutiesProvider;
use std::collections::HashMap;

struct MockDutiesProvider {
    sync_duties: HashMap<(u64, ValidatorIndex), bool>,
    known_epochs: HashSet<Epoch>,
    proposer_duties: HashMap<(Slot, ValidatorIndex), bool>,
    exit_duties: HashMap<(Slot, PublicKeyBytes), u64>,
}

impl DutiesProvider for MockDutiesProvider {
    fn is_validator_in_sync_committee(&self, committee_period: u64, validator_index: ValidatorIndex) -> bool {
        self.sync_duties.get(&(committee_period, validator_index)).copied().unwrap_or(false)
    }
    
    fn is_epoch_known_for_proposers(&self, epoch: Epoch) -> bool {
        self.known_epochs.contains(&epoch)
    }
    
    fn is_validator_proposer_at_slot(&self, slot: Slot, validator_index: ValidatorIndex) -> bool {
        self.proposer_duties.get(&(slot, validator_index)).copied().unwrap_or(false)
    }
    
    fn get_voluntary_exit_duty_count(&self, slot: Slot, pubkey: &PublicKeyBytes) -> u64 {
        self.exit_duties.get(&(slot, *pubkey)).copied().unwrap_or(0)
    }
}

#[tokio::test]
async fn test_duty_checking() {
    let mut mock = MockDutiesProvider::default();
    
    // Set up test data
    let validator_index = ValidatorIndex::from(1);
    mock.sync_duties.insert((100, validator_index), true);
    
    // Test
    assert!(mock.is_validator_in_sync_committee(100, validator_index));
    assert!(!mock.is_validator_in_sync_committee(101, validator_index));
}
```