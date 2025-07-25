# Anchor Validator Store - Usage Examples

## Basic Setup and Initialization

### Creating an AnchorValidatorStore Instance

```rust
use anchor_validator_store::AnchorValidatorStore;
use std::sync::Arc;

// Initialize the validator store with required dependencies
let validator_store = AnchorValidatorStore::new(
    database_state_receiver,           // Database state updates
    signature_collector_manager,       // Distributed signature collection
    qbft_manager,                     // QBFT consensus manager
    slashing_database,                // Slashing protection
    false,                            // Enable slashing protection
    slot_clock,                       // Network time synchronization
    chain_spec,                       // Ethereum consensus spec
    genesis_validators_root,          // Genesis root hash
    Some(rsa_private_key),           // Key share decryption key
    task_executor,                    // Background task management
    30_000_000,                       // Gas limit for proposals
    true,                            // Enable builder proposals
    Some(100),                       // Builder boost factor
    false,                           // Don't prefer builder proposals
    is_synced_receiver,              // Sync status updates
);
```

### Setting up MetadataService

```rust
use anchor_validator_store::metadata_service::MetadataService;

let metadata_service = MetadataService::new(
    duties_service,                   // Validator duties management
    validator_store.clone(),          // Validator store reference
    slot_clock.clone(),              // Time synchronization
    beacon_nodes,                    // Beacon node fallback
    task_executor.clone(),           // Task execution
    chain_spec.clone(),              // Consensus specification
);

// Start the metadata update service
metadata_service.start_update_service()
    .expect("Failed to start metadata service");
```

## ValidatorStore Trait Usage

### Block Signing with Consensus

```rust
use types::{Slot, UnsignedBlock};

async fn sign_validator_block(
    validator_store: &AnchorValidatorStore<T, E>,
    validator_pubkey: PublicKeyBytes,
    unsigned_block: UnsignedBlock<E>,
    current_slot: Slot,
) -> Result<SignedBlock<E>, Error> {
    // The validator store will:
    // 1. Run QBFT consensus on the block
    // 2. Perform slashing protection checks
    // 3. Collect distributed signatures
    // 4. Return the signed block
    validator_store.sign_block(
        validator_pubkey,
        unsigned_block,
        current_slot,
    ).await
}
```

### Attestation Signing with Committee Consensus

```rust
use types::{Attestation, Epoch};

async fn sign_attestation(
    validator_store: &AnchorValidatorStore<T, E>,
    validator_pubkey: PublicKeyBytes,
    validator_committee_position: usize,
    attestation: &mut Attestation<E>,
    current_epoch: Epoch,
) -> Result<(), Error> {
    // The validator store will:
    // 1. Run committee-level QBFT consensus
    // 2. Update attestation data with consensus result
    // 3. Perform slashing protection checks
    // 4. Add signature to attestation
    validator_store.sign_attestation(
        validator_pubkey,
        validator_committee_position,
        attestation,
        current_epoch,
    ).await
}
```

### RANDAO Reveal Generation

```rust
use types::{Epoch, Signature};

async fn produce_randao_reveal(
    validator_store: &AnchorValidatorStore<T, E>,
    validator_pubkey: PublicKeyBytes,
    signing_epoch: Epoch,
) -> Result<Signature, Error> {
    // Produces RANDAO reveal through distributed signature collection
    validator_store.randao_reveal(validator_pubkey, signing_epoch).await
}
```

## Aggregation Operations

### Selection Proof Production

```rust
use types::{Slot, SelectionProof};

async fn produce_selection_proof(
    validator_store: &AnchorValidatorStore<T, E>,
    validator_pubkey: PublicKeyBytes,
    slot: Slot,
) -> Result<SelectionProof, Error> {
    // Times out at 2/3 through the slot to ensure timely aggregation
    validator_store.produce_selection_proof(validator_pubkey, slot).await
}
```

### Aggregate and Proof Signing

```rust
use types::{Attestation, SelectionProof, SignedAggregateAndProof};

async fn create_aggregate_and_proof(
    validator_store: &AnchorValidatorStore<T, E>,
    validator_pubkey: PublicKeyBytes,
    aggregator_index: u64,
    aggregate: Attestation<E>,
    selection_proof: SelectionProof,
) -> Result<SignedAggregateAndProof<E>, Error> {
    // Runs QBFT consensus on the aggregate and proof data
    validator_store.produce_signed_aggregate_and_proof(
        validator_pubkey,
        aggregator_index,
        aggregate,
        selection_proof,
    ).await
}
```

## Sync Committee Operations

### Sync Committee Message Signing

```rust
use types::{Slot, Hash256, SyncCommitteeMessage};

async fn produce_sync_committee_message(
    validator_store: &AnchorValidatorStore<T, E>,
    slot: Slot,
    beacon_block_root: Hash256,
    validator_index: u64,
    validator_pubkey: &PublicKeyBytes,
) -> Result<SyncCommitteeMessage, Error> {
    // Uses committee-level consensus to agree on the beacon block root
    validator_store.produce_sync_committee_signature(
        slot,
        beacon_block_root,
        validator_index,
        validator_pubkey,
    ).await
}
```

### Sync Committee Contribution and Proof

```rust
use types::{SyncCommitteeContribution, SyncSelectionProof, SignedContributionAndProof};

async fn create_sync_contribution_and_proof(
    validator_store: &AnchorValidatorStore<T, E>,
    aggregator_index: u64,
    aggregator_pubkey: PublicKeyBytes,
    contribution: SyncCommitteeContribution<E>,
    selection_proof: SyncSelectionProof,
) -> Result<SignedContributionAndProof<E>, Error> {
    // Handles multi-subnet aggregation with synchronization barriers
    validator_store.produce_signed_contribution_and_proof(
        aggregator_index,
        aggregator_pubkey,
        contribution,
        selection_proof,
    ).await
}
```

## Validator Registration

### MEV-Boost Validator Registration

```rust
use types::{ValidatorRegistrationData, SignedValidatorRegistrationData};

async fn register_validator_for_mev(
    validator_store: &AnchorValidatorStore<T, E>,
    registration_data: ValidatorRegistrationData,
) -> Result<SignedValidatorRegistrationData, Error> {
    // Adjusts timestamp to epoch boundaries and signs with distributed keys
    validator_store.sign_validator_registration_data(registration_data).await
}
```

## Voluntary Exit

### Creating Signed Voluntary Exit

```rust
use types::{VoluntaryExit, SignedVoluntaryExit, Slot};

async fn create_voluntary_exit(
    validator_store: &AnchorValidatorStore<T, E>,
    validator_pubkey: PublicKeyBytes,
    voluntary_exit: VoluntaryExit,
    slot: Slot,
) -> Result<SignedVoluntaryExit, Error> {
    // Uses distributed signature collection for voluntary exit
    validator_store.collect_voluntary_exit_partial_signatures(
        validator_pubkey,
        voluntary_exit,
        slot,
    ).await
}
```

## Configuration Queries

### Getting Validator Configuration

```rust
// Get validator index
let validator_index = validator_store.validator_index(&pubkey);

// Get fee recipient for proposals
let fee_recipient = validator_store.get_fee_recipient(&pubkey);

// Get graffiti for blocks
let graffiti = validator_store.graffiti(&pubkey);

// Get builder boost factor
let boost_factor = validator_store.determine_builder_boost_factor(&pubkey);

// Check if validator can sign (always true for SSV)
let can_sign = validator_store.doppelganger_protection_allows_signing(pubkey);

// Get total number of voting validators
let validator_count = validator_store.num_voting_validators();
```

## Error Handling Patterns

### Handling Timeout Errors

```rust
use anchor_validator_store::{Error, SpecificError};

match validator_store.sign_block(pubkey, block, slot).await {
    Ok(signed_block) => {
        // Block successfully signed after consensus
        process_signed_block(signed_block).await;
    }
    Err(Error::SpecificError(SpecificError::Timeout)) => {
        // Consensus timed out - other operators may be offline
        warn!("Block signing timed out - check operator connectivity");
    }
    Err(Error::SpecificError(SpecificError::NotSynced)) => {
        // Beacon node not synced
        warn!("Cannot sign - beacon node not synchronized");
    }
    Err(Error::Slashable(slashing_error)) => {
        // Slashing protection prevented dangerous signing
        error!("Slashing protection triggered: {:?}", slashing_error);
    }
    Err(error) => {
        // Other errors (signature collection, QBFT, etc.)
        error!("Signing failed: {:?}", error);
    }
}
```

### Handling Missing Validator Data

```rust
match validator_store.validator_index(&pubkey) {
    Some(index) => {
        // Validator is registered and has an index
        perform_validator_duty(index).await;
    }
    None => {
        // Validator not found or missing index
        warn!("Validator {} not found or missing index", pubkey);
    }
}
```

## Performance Monitoring

### Using Metrics

```rust
use anchor_validator_store::metrics;

// Consensus operation timing
let timer = metrics::start_timer_vec(&metrics::CONSENSUS_TIMES, &[metrics::BLOCK]);
let result = perform_consensus_operation().await;
drop(timer); // Automatically records timing

// Success/failure counters are automatically updated by the validator store
// Check logs and metrics endpoints for detailed performance data
```

## Testing and Development

### Disabling Slashing Protection for Tests

```rust
// Create validator store with slashing protection disabled
let test_validator_store = AnchorValidatorStore::new(
    database_state_receiver,
    signature_collector_manager,
    qbft_manager,
    slashing_database,
    true,  // disable_slashing_protection = true
    slot_clock,
    chain_spec,
    genesis_validators_root,
    None,  // No private key needed for impostor mode
    task_executor,
    gas_limit,
    builder_proposals,
    builder_boost_factor,
    prefer_builder_proposals,
    is_synced_receiver,
);
```

This allows testing of signing operations without the safety constraints of slashing protection, useful for development and testing scenarios.