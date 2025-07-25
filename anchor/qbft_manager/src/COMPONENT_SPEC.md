# QBFT Manager Component Specification

## Module Structure

### Core Files
- `lib.rs` - Main QbftManager implementation and traits
- `instance.rs` - Individual QBFT instance lifecycle management
- `timeout.rs` - Round timeout calculation logic
- `tests.rs` - Testing framework and utilities

## API Specification

### QbftManager

#### Constructor
```rust
pub fn new(
    processor: Senders,
    operator_id: OwnOperatorId,
    slot_clock: impl SlotClock + 'static,
    message_sender: Arc<dyn MessageSender>,
    domain: DomainType,
) -> Result<Arc<Self>, QbftError>
```

**Parameters:**
- `processor`: Work distribution channels to central processor
- `operator_id`: This node's operator identifier for consensus participation
- `slot_clock`: Time synchronization interface for cleanup scheduling
- `message_sender`: Network message transmission interface
- `domain`: Network domain type for message construction

**Returns:** `Result<Arc<Self>, QbftError>` - Shared reference to manager or error

#### Public Methods

##### decide_instance
```rust
pub async fn decide_instance<D: QbftDecidable>(
    &self,
    id: D::Id,
    initial: D,
    start_time: Instant,
    committee: &Cluster,
) -> Result<Completed<D>, QbftError>
```

Initiates a new QBFT consensus instance.

**Parameters:**
- `id`: Unique identifier for the instance
- `initial`: Initial consensus data
- `start_time`: When the first round should begin
- `committee`: Committee configuration with member operators

**Returns:** `Result<Completed<D>, QbftError>` - Final consensus result

##### receive_data
```rust
pub fn receive_data(
    &self,
    full_message: SignedSSVMessage,
    qbft_message: ssv_types::consensus::QbftMessage,
) -> Result<(), QbftError>
```

Routes network messages to appropriate QBFT instances.

**Parameters:**
- `full_message`: Complete signed SSV message from network
- `qbft_message`: Decoded QBFT consensus message

**Returns:** `Result<(), QbftError>` - Success or routing error

## Data Structures

### Instance Identifiers

#### ValidatorInstanceId
```rust
pub struct ValidatorInstanceId {
    pub validator: PublicKeyBytes,
    pub duty: ValidatorDutyKind,
    pub instance_height: InstanceHeight,
}
```

Uniquely identifies validator duty consensus instances.

#### CommitteeInstanceId
```rust
pub struct CommitteeInstanceId {
    pub committee: CommitteeId,
    pub instance_height: InstanceHeight,
}
```

Uniquely identifies committee consensus instances.

### Message Types

#### QbftMessage
```rust
pub struct QbftMessage<D: QbftData> {
    pub kind: QbftMessageKind<D>,
    pub drop_on_finish: Option<DropOnFinish>,
}
```

Internal message wrapper with lifecycle management.

#### QbftMessageKind
```rust
pub enum QbftMessageKind<D: QbftData> {
    Initialize(QbftInitialization<D>),
    NetworkMessage(WrappedQbftMessage),
}
```

Message payload types:
- `Initialize`: Creates new consensus instance
- `NetworkMessage`: Forwards network consensus message

#### QbftInitialization
```rust
pub struct QbftInitialization<D: QbftData> {
    initial: D,
    message_id: MessageId,
    start_time: Instant,
    config: qbft::Config<DefaultLeaderFunction>,
    on_completed: oneshot::Sender<Completed<D>>,
}
```

Contains all data needed to start a new QBFT instance.

## Traits

### QbftDecidable
```rust
pub trait QbftDecidable: QbftData<Hash = Hash256> + Send + Sync + 'static {
    type Id: Hash + Eq + Send + Debug;
    
    fn get_map(manager: &QbftManager) -> &Map<Self::Id, Self>;
    fn get_or_spawn_instance(manager: &QbftManager, id: Self::Id) -> UnboundedSender<QbftMessage<Self>>;
    fn instance_height(&self, id: &Self::Id) -> InstanceHeight;
    fn message_id(domain: &DomainType, id: &Self::Id) -> MessageId;
}
```

Defines requirements for data types that can participate in QBFT consensus.

**Methods:**
- `get_map`: Returns storage map for this data type
- `get_or_spawn_instance`: Gets existing or creates new instance sender
- `instance_height`: Extracts consensus height from identifier
- `message_id`: Constructs message ID for network messages

## Instance State Machine

### States
1. **Uninitialized** - Buffers messages until initialization
2. **Initialized** - Active consensus with timeout management  
3. **Decided** - Consensus complete, result available

### Transitions
- Uninitialized → Initialized: Via initialization message
- Initialized → Decided: When consensus reaches completion
- Any → Closed: When instance receiver is dropped

## Configuration

### Timeout Parameters (`timeout.rs`)
```rust
const QUICK_TIMEOUT_THRESHOLD: u64 = 8;  // Round 8
const QUICK_TIMEOUT: u64 = 2;            // 2 seconds
const SLOW_TIMEOUT: u64 = 120;           // 2 minutes
```

Timeout calculation:
- Rounds 1-8: `round * 2 seconds`  
- Rounds 9+: `16 seconds + (round - 8) * 2 minutes`

### Instance Management
```rust
const QBFT_RETAIN_SLOTS: u64 = 1;        // Slots to retain before cleanup
const MESSAGE_BUFFER_LIMIT: usize = 100; // Max buffered messages per instance
```

## Error Types

### QbftError
```rust
pub enum QbftError {
    QueueClosedError,              // Processor queue closed
    QueueFullError,                // Processor queue full
    ConfigBuilderError(ConfigBuilderError), // Invalid QBFT configuration
    InconsistentMessageId,         // Malformed message identifier
    OwnOperatorIdUnknown,          // Node operator ID not set
}
```

## Dependencies

### External Crates
- `dashmap`: Concurrent hash maps for instance storage
- `tokio`: Async runtime and synchronization primitives
- `tracing`: Structured logging and diagnostics
- `ethereum_ssz`: Serialization for Ethereum data types

### Internal Dependencies
- `database`: Operator ID management
- `message_sender`: Network message transmission
- `processor`: Work queue and task execution
- `qbft`: Core QBFT consensus algorithm
- `slot_clock`: Time synchronization
- `ssv_types`: SSV protocol data types
- `types`: Ethereum-specific types

## Thread Safety

The `QbftManager` is thread-safe and designed for concurrent access:
- Uses `Arc<Self>` for shared ownership
- `DashMap` provides concurrent access to instance maps
- Message passing via `mpsc` channels for inter-task communication
- Atomic operations where needed for consistency