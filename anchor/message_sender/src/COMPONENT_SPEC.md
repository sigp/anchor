# Message Sender Component Technical Specification

## Module Structure

```
message_sender/
├── lib.rs           # Core trait and error definitions
├── network.rs       # Production NetworkMessageSender implementation
├── impostor.rs      # Testing ImpostorMessageSender implementation
└── testing.rs       # MockMessageSender for unit tests
```

## Core Interfaces

### MessageSender Trait
```rust
pub trait MessageSender: Send + Sync {
    fn sign_and_send(
        &self,
        message: UnsignedSSVMessage,
        committee_id: CommitteeId,
        additional_message_callback: Option<Box<MessageCallback>>,
    ) -> Result<(), Error>;
    
    fn send(
        &self, 
        message: SignedSSVMessage, 
        committee_id: CommitteeId
    ) -> Result<(), Error>;
}
```

### Error Types
```rust
pub enum Error {
    Processor(processor::Error),    // Consensus processor issues
    NetworkQueueClosed,             // Network layer unavailable
    OwnOperatorIdUnknown,          // Missing operator identity
    NotSynced,                     // Node not synchronized
}
```

### Message Callback Type
```rust
type MessageCallback = dyn FnOnce(&SignedSSVMessage) + Send + 'static;
```

## Implementation Specifications

### NetworkMessageSender

#### Constructor Parameters
- `processor: processor::Senders` - Consensus processing queues
- `network_tx: mpsc::Sender<(SubnetId, Vec<u8>)>` - Network transmission channel
- `private_key: Rsa<Private>` - RSA private key for signing
- `operator_id: OwnOperatorId` - This node's operator identifier
- `validator: Option<Arc<Validator<S, D>>>` - Optional message validator
- `subnet_count: usize` - Total number of network subnets
- `is_synced: watch::Receiver<bool>` - Synchronization status receiver

#### Key Methods

**`sign_and_send()`**
- Validates network channel availability
- Checks operator ID availability
- Verifies node synchronization status
- Delegates to consensus processor with signing closure
- Generates RSA signature using SHA-256 digest
- Creates SignedSSVMessage with signature and operator ID
- Executes optional message callback
- Forwards to `do_send()` for network transmission

**`send()`**
- Validates network channel and sync status
- Delegates signed message to consensus processor
- Forwards to `do_send()` for network transmission

**`do_send()` (Private)**
- Serializes message to SSZ bytes
- Performs optional message validation
- Calculates target subnet from committee ID
- Attempts non-blocking send to network channel
- Handles channel errors (closed/full) with appropriate logging

**`sign()` (Private)**
- Creates SHA-256 signer with private key
- Updates signer with serialized SSV message
- Returns signature bytes

#### Validation Integration
- Optional message validation before network transmission
- Distinguishes between `Reject` (severe) and `Ignore` (temporary) errors
- Logs validation failures with appropriate severity levels

### ImpostorMessageSender

#### Purpose
Testing implementation that simulates message sending without network transmission.

#### Key Features
- Maintains network channel reference to prevent closure errors
- Calculates subnet routing for realistic behavior
- Logs debug messages showing what would be sent
- Always returns success for testing scenarios

### MockMessageSender

#### Purpose
Unit testing utility that captures messages for verification.

#### Constructor Parameters
- `message_tx: mpsc::UnboundedSender<SignedSSVMessage>` - Message capture channel
- `operator_id: OperatorId` - Mock operator identifier

#### Key Features
- Generates dummy RSA signatures (zero-filled bytes)
- Captures all sent messages in unbounded channel
- Supports message callbacks for testing
- Returns network queue errors if capture channel closes

## Cryptographic Specifications

### Signature Algorithm
- **Algorithm**: RSA with PKCS#1 v1.5 padding
- **Hash Function**: SHA-256
- **Key Type**: RSA private key via OpenSSL
- **Signature Size**: Variable (typically 256 bytes for 2048-bit keys)

### Message Serialization
- **Format**: SSZ (Simple Serialize)
- **Input**: `SSVMessage` component of `UnsignedSSVMessage`
- **Process**: Serialize → Hash → Sign

## Network Integration

### Subnet Routing
- **Calculation**: `SubnetId::from_committee(committee_id, subnet_count)`
- **Distribution**: Committee IDs mapped to subnets for load balancing
- **Channel Format**: `(SubnetId, Vec<u8>)` tuples

### Channel Management
- **Type**: `tokio::sync::mpsc::Sender`
- **Backpressure**: Non-blocking `try_send()` with full queue handling
- **Error Handling**: Distinguishes between closed and full channel states

## Async Processing

### Processor Integration
- **Queue**: `processor.urgent_consensus` for high-priority messages
- **Execution**: `send_blocking()` with named tasks for monitoring
- **Task Names**: 
  - `"message_sign_and_send"` for signing operations
  - `"message_send"` for direct sending

### Error Propagation
- Processor errors bubble up through `Error::Processor` variant
- Network errors handled locally with logging
- Validation errors prevent message transmission

## Synchronization Requirements

### Node Sync Status
- **Check**: `*is_synced.borrow()` before message processing
- **Error**: Returns `Error::NotSynced` if node not synchronized
- **Rationale**: Prevents message transmission during initial sync

### Operator Identity
- **Check**: `operator_id.get()` availability
- **Error**: Returns `Error::OwnOperatorIdUnknown` if missing
- **Rationale**: Ensures proper message attribution

## Memory and Resource Management

### Arc Usage
- NetworkMessageSender wrapped in `Arc` for shared ownership
- Enables cloning for async task execution
- Validator also wrapped in `Arc` for shared access

### Channel Resources
- Network channel sender cloned for lifetime management
- Processor queues manage their own resource cleanup
- Private keys managed through OpenSSL's memory management