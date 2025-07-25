# Message Receiver Component Specification

## API Reference

### Traits

#### `MessageReceiver`
**Location**: `lib.rs:9-16`

```rust
pub trait MessageReceiver {
    fn receive(
        &self,
        propagation_source: PeerId,
        message_id: MessageId,
        message: Message,
    ) -> Result<(), Error>;
}
```

**Parameters**:
- `propagation_source: PeerId` - The peer that sent us this message
- `message_id: MessageId` - Unique identifier for the gossipsub message
- `message: Message` - The actual gossipsub message containing SSV data

**Returns**: `Result<(), Error>` - Success or processor error

### Structs

#### `NetworkMessageReceiver<S, D>`
**Location**: `manager.rs:27-34`

**Generic Parameters**:
- `S: SlotClock + 'static` - Clock implementation for timing
- `D: DutiesProvider` - Provider for validator duty information

**Fields**:
- `processor: processor::Senders` - Channel senders for consensus processing
- `qbft_manager: Arc<QbftManager>` - QBFT consensus manager
- `signature_collector: Arc<SignatureCollectorManager>` - Signature aggregation manager
- `network_state_rx: watch::Receiver<NetworkState>` - Network state updates
- `outcome_tx: mpsc::Sender<Outcome>` - Channel for validation outcomes
- `validator: Arc<Validator<S, D>>` - Message validator

#### `Outcome`
**Location**: `manager.rs:20-24`

```rust
pub struct Outcome {
    pub message_id: MessageId,
    pub propagation_source: PeerId,
    pub action: MessageAcceptance,
}
```

**Fields**:
- `message_id: MessageId` - Gossipsub message identifier
- `propagation_source: PeerId` - Source peer of the message
- `action: MessageAcceptance` - Accept/reject decision for gossipsub

### Enums

#### `Error`
**Location**: `lib.rs:18-22`

```rust
#[derive(Error, Debug)]
pub enum Error {
    #[error("Processor error: {0}")]
    Processor(#[from] processor::Error),
}
```

Wraps processor errors that can occur during message handling.

## Implementation Details

### Constructor

#### `NetworkMessageReceiver::new`
**Location**: `manager.rs:37-53`

```rust
pub fn new(
    processor: processor::Senders,
    qbft_manager: Arc<QbftManager>,
    signature_collector: Arc<SignatureCollectorManager>,
    network_state_rx: watch::Receiver<NetworkState>,
    outcome_tx: mpsc::Sender<Outcome>,
    validator: Arc<Validator<S, D>>,
) -> Arc<Self>
```

Creates a new `NetworkMessageReceiver` wrapped in an `Arc` for shared ownership.

### Message Processing Pipeline

#### Phase 1: Validation (`manager.rs:71`)
- Messages are validated using the injected validator
- Validation results determine message acceptance/rejection

#### Phase 2: Outcome Reporting (`manager.rs:73-86`)
- Validation outcomes are sent via `outcome_tx` channel
- Errors are logged if the outcome channel is closed or full
- Network layer uses outcomes for gossipsub feedback

#### Phase 3: Interest Filtering (`manager.rs:109-149`)

**Validator Messages** (`manager.rs:110-122`):
- Extracts validator ID from message
- Checks if we have shares for this validator in network state
- Early returns if not interested

**Committee Messages** (`manager.rs:123-144`):
- Extracts committee ID from message
- Verifies our operator ID is a member of the committee
- Uses cluster membership data for verification

#### Phase 4: Message Routing (`manager.rs:151-168`)

**QBFT Messages** (`manager.rs:152-159`):
- Routes to `qbft_manager.receive_data()`
- Errors are logged but don't fail the entire process

**Partial Signature Messages** (`manager.rs:160-167`):
- Routes to `signature_collector.receive_partial_signatures()`
- Errors are logged but don't fail the entire process

## Constants

- `RECEIVER_NAME: &str = "message_receiver"` - Identifier for processor channels

## Threading and Concurrency

### Async Processing
- All message processing occurs asynchronously via `processor.urgent_consensus.send_blocking()`
- Uses a closure to capture necessary data and process in background
- Maintains responsiveness of the network layer

### Channel Types
- `watch::Receiver<NetworkState>` - Single-producer, multiple-consumer for network state
- `mpsc::Sender<Outcome>` - Multiple-producer, single-consumer for validation outcomes
- Processor channels for async consensus handling

## Error Handling Strategy

### Validation Failures
- `PreDecodeFailure` - Message couldn't be decoded, logged and dropped
- `PostDecodeFailure` - Message decoded but validation failed, logged and dropped

### Channel Errors
- Outcome channel closed/full - Logged as errors but processing continues
- Manager errors - Logged with context but don't crash the receiver

### Logging Levels
- `trace` - Uninteresting messages (filtered out)
- `debug` - Validation failures with message content
- `error` - Channel failures and manager errors

## Performance Considerations

### Early Returns
- Messages for uninteresting validators/committees are filtered early
- Reduces unnecessary processing load

### Async Processing
- Non-blocking message handling prevents network layer bottlenecks
- Urgent consensus queue ensures critical messages are prioritized

### Memory Management
- Uses `Arc` for shared ownership without copying large structures
- Clones only when necessary for async processing

## Dependencies and Integration Points

### Required Traits
- `S: SlotClock + 'static` - For timing coordination
- `D: DutiesProvider` - For validator duty information

### External Interfaces
- Gossipsub network layer (input)
- QBFT manager (output)
- Signature collector (output)
- Processor urgent consensus (execution)
- Network state provider (configuration)