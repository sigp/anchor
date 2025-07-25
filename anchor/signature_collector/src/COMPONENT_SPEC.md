# Signature Collector - Technical Specification

## API Reference

### Primary Interface

#### `SignatureCollectorManager::new()`
```rust
pub fn new(
    processor: Senders,
    operator_id: OwnOperatorId,
    domain: DomainType,
    message_sender: Arc<dyn MessageSender>,
    slot_clock: impl SlotClock + 'static,
) -> Result<Arc<Self>, CollectionError>
```
Creates a new signature collector manager instance with automatic cleanup task.

#### `SignatureCollectorManager::sign_and_collect()`
```rust
pub async fn sign_and_collect(
    self: &Arc<Self>,
    metadata: SignatureMetadata,
    requester: SignatureRequester,
    signing_data: SigningData,
) -> Result<Arc<Signature>, CollectionError>
```
Main entry point for signature collection. Signs locally and waits for threshold signatures.

#### `SignatureCollectorManager::receive_partial_signatures()`
```rust
pub fn receive_partial_signatures(
    self: &Arc<Self>,
    messages: PartialSignatureMessages,
) -> Result<(), CollectionError>
```
Processes incoming partial signatures from network peers.

### Data Structures

#### `SignatureMetadata`
```rust
pub struct SignatureMetadata {
    pub kind: PartialSignatureKind,    // Signature type (attestation, proposal, etc.)
    pub role: Role,                    // Validator role in consensus
    pub threshold: u64,                // Minimum signatures needed
    pub slot: Slot,                    // Target slot for signature
    pub committee_id: CommitteeId,     // Committee identifier
}
```

#### `SignatureRequester`
```rust
pub enum SignatureRequester {
    SingleValidator { pubkey: PublicKeyBytes },
    Committee { num_signatures_to_collect: usize },
}
```

#### `SigningData`
```rust
pub struct SigningData {
    pub root: Hash256,              // Data to be signed
    pub index: ValidatorIndex,      // Validator index
    pub share: Option<SecretKey>,   // BLS key share (None for impostor mode)
}
```

## Internal Architecture

### Collector Instance Lifecycle

1. **Creation**: `get_or_spawn()` creates new collector or returns existing one
2. **Registration**: Tasks register with collector via `RegisterNotifier` message
3. **Collection**: Partial signatures arrive via `PartialSignature` messages
4. **Reconstruction**: When threshold met, signatures combined using Lagrange interpolation
5. **Notification**: All registered tasks receive reconstructed signature
6. **Cleanup**: Instance removed after `SIGNATURE_COLLECTOR_RETAIN_SLOTS`

### Message Types

#### `CollectorMessageKind`
```rust
enum CollectorMessageKind {
    RegisterNotifier {
        notify: oneshot::Sender<Arc<Signature>>,
        threshold: u64,
    },
    PartialSignature {
        operator_id: OperatorId,
        signature: Box<Signature>,
    },
}
```

### Threading Model

- **Permitless Queue**: General message processing and instance management
- **Urgent Consensus Queue**: Time-critical signature creation
- **Async Tasks**: Individual collector instances and cleanup task

## Configuration

### Constants
- `SIGNATURE_COLLECTOR_RETAIN_SLOTS`: 1 slot retention period
- Task names for processor queues:
  - `COLLECTOR_NAME`: "signature_collector"
  - `COLLECTOR_MESSAGE_NAME`: "signature_collector_message"
  - `COLLECTOR_CLEANER_NAME`: "signature_collector_cleaner"
  - `SIGNER_NAME`: "partial_signer"

## Error Conditions

### `CollectionError` Variants

| Error | Description | Recovery |
|-------|-------------|----------|
| `QueueClosedError` | Processor queue closed | System shutdown |
| `QueueFullError` | Processor queue full | Backpressure/retry |
| `CollectionTimeout` | Signature collection timeout | Retry with new instance |
| `EmptySignature` | Invalid empty signature | Check operator configuration |
| `OwnOperatorIdUnknown` | Local operator ID not set | Configure operator identity |
| `RecoverError(bls_lagrange::Error)` | BLS signature recovery failed | Check threshold/signatures |

## Performance Characteristics

### Memory Usage
- `O(n)` where n = number of active signature requests
- Bounded by slot-based cleanup (1 slot retention)
- DashMap provides concurrent access without global locks

### Network Impact
- Sends 1 message per validator signature (SingleValidator mode)
- Sends 1 message per committee (Committee mode)
- Receives partial signatures from all committee operators

### CPU Usage
- BLS signature operations are computationally intensive
- Lagrange interpolation for signature reconstruction
- Concurrent processing via tokio async runtime

## Security Properties

### Threshold Security
- Requires `threshold` out of `n` operators to reconstruct signature
- Byzantine fault tolerance: tolerates up to `n - threshold` malicious operators
- Cryptographically secure BLS threshold signatures

### Attack Resistance
- Duplicate signature detection and rejection
- Conflicting signature detection with error logging
- Timeout-based cleanup prevents resource exhaustion

## Dependencies and Integration

### Required Dependencies
- `bls_lagrange`: ^0.1.0 (BLS operations)
- `dashmap`: ^5.0.0 (concurrent maps)
- `tokio`: ^1.0.0 (async runtime)

### Integration Points
- `MessageSender`: Network message transmission
- `Processor`: Task queue management
- `SlotClock`: Time-based operations
- `Database`: Operator identity storage

## Testing Considerations

- Mock `MessageSender` for network testing
- Synthetic `SlotClock` for time-based testing
- Multiple operator simulation for threshold testing
- Error injection for failure scenario testing
- Concurrent access testing for race conditions