# QBFT Manager AI Documentation

## Overview

The QBFT Manager is a critical component of the Anchor SSV (Secret Shared Validator) system that orchestrates QBFT (Quadratic Byzantium Fault Tolerant) consensus instances. It manages multiple concurrent QBFT consensus processes for different types of validator duties and committee operations.

## Architecture

### Core Structure

The `QbftManager` serves as the central orchestrator with these key responsibilities:

1. **Instance Management**: Maintains maps of active QBFT instances for different consensus types
2. **Message Routing**: Routes network messages to appropriate consensus instances
3. **Lifecycle Management**: Spawns new instances and cleans up old ones
4. **Configuration**: Builds QBFT configurations with proper committee membership and quorum rules

### Key Components

#### QbftManager (`lib.rs:107-147`)
The main manager struct containing:
- `processor`: Senders for work distribution to the central processor
- `operator_id`: This node's operator identifier
- `validator_consensus_data_instances`: Map of validator duty consensus instances
- `beacon_vote_instances`: Map of committee voting instances
- `message_sender`: Network message sending interface
- `domain`: Network domain for message construction

#### Instance Types
Two types of QBFT instances are managed:

1. **Validator Consensus Data** (`ValidatorInstanceId`)
   - Handles proposal, aggregation, and sync committee duties
   - Keyed by validator public key, duty type, and instance height

2. **Beacon Vote** (`CommitteeInstanceId`)
   - Handles committee-level consensus
   - Keyed by committee ID and instance height

#### Instance Lifecycle (`instance.rs`)

Each QBFT instance goes through these states:
- **Uninitialized**: Buffers incoming messages until initialization
- **Initialized**: Active consensus with timeout management
- **Decided**: Consensus complete, result available

### Message Flow

1. **Initialization**: `decide_instance()` creates new consensus with initial data
2. **Network Messages**: `receive_data()` routes messages to appropriate instances
3. **Internal Processing**: Instances handle consensus rounds with timeouts
4. **Completion**: Results sent back via oneshot channels

### Timeout Strategy (`timeout.rs`)

Implements exponential backoff with two phases:
- **Quick Timeout**: First 8 rounds use 2-second increments
- **Slow Timeout**: Later rounds use 2-minute increments

## Key Features

### Concurrent Instance Management
- Multiple QBFT instances run concurrently for different duties
- Each instance is independent with its own consensus state
- Automatic cleanup of old instances based on slot progression

### Fault Tolerance
- Byzantine fault tolerance through QBFT consensus algorithm
- Message buffering for uninitialized instances
- Graceful handling of dropped messages when buffers are full

### Integration Points
- **Processor**: Work queue integration for async task execution
- **MessageSender**: Network layer for message distribution
- **SlotClock**: Time synchronization for instance cleanup
- **Database**: Operator ID management

## State Management

The manager maintains two primary state maps:
- `validator_consensus_data_instances`: For individual validator duties
- `beacon_vote_instances`: For committee-level decisions

Instances are automatically cleaned up when they become stale (more than 1 slot old) to prevent memory leaks.

## Error Handling

Comprehensive error handling through `QbftError` enum:
- Queue management errors (full/closed)
- Configuration validation errors
- Message ID consistency errors
- Operator ID availability errors

## Testing Support

Extensive testing framework (`tests.rs`) provides:
- Mock message senders for testing
- Manual slot clock control
- Test context management
- Timeout handling for test scenarios