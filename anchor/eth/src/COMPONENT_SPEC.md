# Eth Component - Technical Specifications

## Module Structure

```
eth/src/
├── lib.rs              # Public API exports
├── sync.rs             # Main synchronization orchestrator  
├── event_processor.rs  # SSV contract event processing logic
├── event_parser.rs     # Event decoding and parsing utilities
├── index_sync.rs       # Validator index synchronization
├── voluntary_exit_processor.rs # Validator exit handling
├── error.rs            # Error types and handling
├── metrics.rs          # Performance metrics collection
├── util.rs             # Utility functions and helpers
└── generated.rs        # Auto-generated contract bindings
```

## Public API

### Main Types

```rust
pub use sync::{Config, SsvEventSyncer};
pub mod index_sync;
pub mod voluntary_exit_processor;
pub use metrics::EXECUTION_EVENTS_PROCESSED;
```

### Configuration

```rust
pub struct Config {
    pub http_urls: Vec<SensitiveUrl>,  // HTTP RPC endpoints with fallback
    pub ws_url: SensitiveUrl,          // WebSocket RPC endpoint
    pub network: SsvNetworkConfig,     // Network-specific configuration
}
```

## Core Components Specification

### SsvEventSyncer

**Purpose**: Main orchestrator for Ethereum event synchronization

**Key Methods**:
- `new()` - Creates syncer for node operation with full event processing
- `new_keysplit()` - Creates syncer for key split operations (limited events)
- `start_sync()` - Begins synchronization process
- `keysplit_sync()` - Performs historical sync for key splits only

**Internal State**:
- `rpc_client: RootProvider` - HTTP client for RPC calls
- `ws_client: RootProvider` - WebSocket client for real-time events
- `event_processor: EventProcessor` - Handles event processing logic
- `network: SsvNetworkConfig` - Network configuration
- `is_synced: watch::Sender<bool>` - Sync status notifications

### EventProcessor

**Purpose**: Processes and stores SSV contract events

**Processing Modes**:
```rust
pub enum Mode {
    Node {
        index_sync_tx: index_sync::Tx,  // Validator index sync queue
        exit_tx: ExitTx,                // Exit processing queue
    },
    KeySplit,  // Limited processing for key splits
}
```

**Event Types Handled**:
- `OperatorAdded` - New operator registration
- `OperatorRemoved` - Operator deregistration  
- `ValidatorAdded` - Validator registration with cluster
- `ValidatorRemoved` - Validator removal from cluster
- `ClusterLiquidated` - Cluster liquidation event
- `ClusterReactivated` - Cluster reactivation event
- `FeeRecipientAddressUpdated` - Fee recipient changes

### Error Types

```rust
pub enum ExecutionError {
    SyncError(String),     // Synchronization failures
    InvalidEvent(String),  // Event parsing/validation errors
    RpcError(String),      // RPC communication errors
    WsError(String),       // WebSocket connection errors
    DecodeError(String),   // Event decoding errors
    Misc(String),          // General errors
    Duplicate(String),     // Duplicate event handling
    Database(String),      // Database operation errors
}
```

## Event Processing Pipeline

### 1. Event Collection
- WebSocket connection monitors contract for new events
- HTTP fallback used when WebSocket fails
- Events filtered by contract address and signature

### 2. Event Decoding
- Raw `alloy::rpc::types::Log` converted to typed events
- Contract ABI used for proper decoding
- Validation ensures event structure integrity

### 3. Event Processing
- Events processed based on type and current mode
- Database transactions ensure atomicity
- State updates maintain consistency

### 4. Side Effects
- New validators trigger index sync requests
- Validator exits queued for processing
- Metrics updated for monitoring

## Database Integration

### Tables Modified
- `operators` - Operator state and metadata
- `validators` - Validator registrations and status
- `clusters` - Cluster state and composition
- `fee_recipients` - Fee recipient mappings

### Transaction Patterns
- Single transaction per event batch
- Rollback on any processing error
- Conflict resolution for concurrent updates

## Network Communication

### WebSocket Connection
- Primary connection for real-time events
- Automatic reconnection on disconnect
- Heartbeat monitoring for connection health

### HTTP Fallback
- Used when WebSocket unavailable
- Polling-based event collection
- Multiple endpoint fallback support

### RPC Methods Used
- `eth_getLogs` - Historical event retrieval
- `eth_subscribe` - Real-time event streaming
- `eth_getBlockByNumber` - Block metadata

## Performance Characteristics

### Memory Usage
- Streaming event processing (no large buffers)
- Connection pooling for HTTP clients
- Bounded queues for cross-component communication

### Processing Throughput
- Batch processing for efficiency
- Configurable batch sizes
- Parallel processing where possible

### Error Recovery
- Exponential backoff for failed requests
- Circuit breaker pattern for failing endpoints
- Graceful degradation to HTTP when WebSocket fails

## Metrics and Monitoring

### Key Metrics
- `EXECUTION_EVENTS_PROCESSED` - Total events processed
- `EXECUTION_SYNC_STATUS` - Current sync status (0/1)
- Connection health and error rates
- Processing latency and throughput

### Health Checks
- WebSocket connection status
- Database connectivity
- Event processing lag
- RPC endpoint reliability

## Security Considerations

### Input Validation
- All incoming events validated before processing
- Contract address verification
- Event signature verification

### Database Security
- Prepared statements prevent SQL injection
- Transaction isolation prevents race conditions
- Input sanitization for all user data

### Network Security
- TLS required for all RPC connections
- Authentication tokens secured
- Rate limiting on outbound requests