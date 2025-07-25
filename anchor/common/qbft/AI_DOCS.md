# QBFT Consensus Component - AI Documentation

## Overview

This component implements a **Quorum Based Fault Tolerance (QBFT)** consensus algorithm for the Anchor SSV (Secret Shared Validator) project. QBFT is a Byzantine fault-tolerant consensus protocol that allows a distributed system to reach agreement on a value despite some nodes being faulty or malicious.

## Core Architecture

### Main Components

1. **`Qbft<F, D, S>` struct** (`lib.rs:80-128`) - The main consensus instance
   - Manages the entire QBFT consensus process
   - Handles message reception, validation, and state transitions
   - Generic over leader function `F`, data type `D`, and message sender `S`

2. **Message Containers** (`msg_container.rs`) - Storage for different message types
   - `propose_container` - Stores proposal messages
   - `prepare_container` - Stores prepare messages  
   - `commit_container` - Stores commit messages
   - `round_change_container` - Stores round change messages

3. **Configuration** (`config.rs`) - QBFT instance configuration
   - Committee members, quorum size, operator ID
   - Leader function, instance height, round limits

4. **QBFT Types** (`qbft_types.rs`) - Core data structures
   - Message wrappers, instance states, completion status
   - Leader functions for determining round leaders

## Consensus Flow

### 1. Initialization (`lib.rs:143-185`)
- Creates new QBFT instance with start data
- Sets up message containers with quorum requirements
- Begins first round if node is leader

### 2. Message Processing Pipeline

#### Proposal Phase (`lib.rs:414-471`)
- Leader proposes data for the round
- Validates justifications for proposals beyond first round
- Transitions to Prepare state upon accepting proposal

#### Prepare Phase (`lib.rs:589-662`)
- Nodes send prepare messages for accepted proposals
- Reaches prepare consensus when quorum achieved
- Records consensus and transitions to Commit state

#### Commit Phase (`lib.rs:664-735`)
- Nodes send commit messages for prepared values
- Aggregates commit messages when quorum reached
- Marks instance as complete upon commit consensus

#### Round Change (`lib.rs:770-821`)
- Handles round timeouts and failures
- Collects round change messages for quorum
- Advances to new round with proper justifications

### 3. Message Validation (`lib.rs:234-310`)
- Validates message rounds, instance heights, committee membership
- Deserializes and validates full data when present
- Ensures message signatures are from valid committee members

## Key Features

### Byzantine Fault Tolerance
- Tolerates up to `f = (n-1)/3` Byzantine failures
- Requires `2f+1` messages for quorum consensus
- Validates all message justifications and signatures

### Round-based Consensus
- Uses rounds to handle network delays and failures
- Automatic round progression on timeouts
- Leader rotation using configurable leader functions

### Message Aggregation (`lib.rs:737-768`)
- Aggregates multiple commit signatures into single message
- Reduces network overhead for final consensus proof
- Maintains full data in aggregated messages

### Justification System (`lib.rs:479-587`)
- Validates round change justifications for proposals
- Ensures prepare justifications match proposed values
- Prevents equivocation and maintains safety

## Data Flow

```
Start Data → Proposal → Prepare Consensus → Commit Consensus → Completion
     ↓           ↓            ↓                ↓
   Leader    Validate     Record Past      Aggregate
  Election  Justif.      Consensus        Messages
```

## State Management

The QBFT instance maintains several critical states:

- **`current_round`** - Current consensus round
- **`state`** - Instance state (AwaitingProposal, Prepare, Commit, etc.)
- **`past_consensus`** - Historical prepare consensus for justifications
- **`data`** - Map of all seen consensus data by hash
- **`completed`** - Final consensus result (Success/TimedOut)

## Integration Points

### Message Sender Interface (`lib.rs:62-70`)
- Generic trait for sending unsigned messages
- Allows flexible integration with network layers
- Supports both closure and custom implementations

### Data Validation
- Generic `QbftData` trait for consensus data types
- Hash-based data identification and validation
- SSZ serialization for network transmission

## Error Handling

- Comprehensive message validation with detailed logging
- Graceful handling of invalid/duplicate messages
- Timeout mechanisms for round progression
- State consistency checks throughout consensus

## Testing

The component includes comprehensive tests in `tests.rs` covering:
- Basic consensus scenarios
- Round change handling  
- Byzantine failure tolerance
- Message validation edge cases

## Configuration Options

- **Quorum Size** - Number of messages needed for consensus
- **Max Rounds** - Maximum rounds before timeout
- **Committee Members** - Set of valid consensus participants
- **Leader Function** - Algorithm for leader selection per round