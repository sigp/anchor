# Eth Component - AI Documentation

## Overview

The `eth` component is responsible for synchronizing with the Ethereum execution layer to track SSV (Secret Shared Validator) contract events and maintain validator state. It serves as the bridge between the Ethereum blockchain and the SSV network node, ensuring that all relevant contract events are processed and stored locally.

## Architecture

The component follows an event-driven architecture with the following key modules:

### Core Components

1. **SsvEventSyncer** (`sync.rs`): Main orchestrator that manages the synchronization process
2. **EventProcessor** (`event_processor.rs`): Processes different types of SSV contract events
3. **EventDecoder** (`event_parser.rs`): Decodes raw Ethereum logs into structured events
4. **IndexSync** (`index_sync.rs`): Handles validator index synchronization with beacon chain
5. **VoluntaryExitProcessor** (`voluntary_exit_processor.rs`): Manages validator exit operations

### Key Features

- **Real-time Event Monitoring**: Continuously monitors SSV contract for new events
- **Historical Sync**: Can synchronize from a specific block height to catch up with past events
- **Error Handling**: Robust error handling with retry mechanisms for network failures
- **Metrics Collection**: Comprehensive metrics for monitoring sync status and performance
- **Fallback Support**: HTTP fallback mechanisms for WebSocket connection failures

### Event Types Processed

The component tracks these critical SSV contract events:
- `OperatorAdded` / `OperatorRemoved`: Operator lifecycle management
- `ValidatorAdded` / `ValidatorRemoved`: Validator registration and removal
- `ClusterLiquidated` / `ClusterReactivated`: Cluster state changes
- `FeeRecipientAddressUpdated`: Fee recipient updates

### Data Flow

1. **Event Collection**: WebSocket connection to Ethereum node collects real-time logs
2. **Event Parsing**: Raw logs are decoded into structured Rust types
3. **Event Processing**: Events are processed based on their type and stored in database
4. **State Updates**: Local validator and operator state is updated accordingly
5. **Index Sync**: New validators trigger beacon chain index lookups
6. **Exit Processing**: Validator exits are queued for processing

### Dependencies

- **Alloy**: Ethereum client library for RPC communication and event handling
- **Database**: Local SQLite database for storing validator and operator state
- **SSV Types**: Shared types for SSV network components
- **Tokio**: Async runtime for concurrent event processing

## Configuration

The component is configured through the `Config` struct which includes:
- Ethereum RPC endpoints (WebSocket and HTTP fallbacks)
- SSV contract address and deployment block
- Sync parameters (block ranges, retry intervals)
- Processing modes (Node vs. Bootstrap)

## Error Handling

The component implements comprehensive error handling for:
- Network connectivity issues
- RPC endpoint failures
- Contract event parsing errors
- Database transaction failures
- Concurrent access patterns

## Performance Considerations

- Uses batched event processing for efficiency
- Implements connection pooling for HTTP fallbacks
- Employs async/await patterns for non-blocking operations
- Provides configurable batch sizes and timeout values