# Scoring Component Documentation

## Overview

The scoring component implements peer and topic scoring mechanisms for SSV (Secret Shared Validator) gossipsub networks. It provides dynamic reputation management to maintain network health and prevent malicious behavior by calculating scores based on peer interactions and message patterns.

## Architecture

The scoring system consists of three main modules:

1. **`peer_score_config.rs`** - Implements peer-level scoring parameters and thresholds
2. **`topic_score_config.rs`** - Implements topic-specific scoring configuration with dynamic adaptation
3. **`mod.rs`** - Provides utility functions for decay calculations and module exports

## Core Concepts

### Peer Scoring
Peer scoring evaluates individual peers based on their overall network behavior:
- **Gossip Threshold** (-4000.0): Minimum score to participate in gossip
- **Publish Threshold** (-8000.0): Minimum score to publish messages
- **Graylist Threshold** (-16000.0): Score below which peers are rejected
- **Accept PX Threshold** (100.0): Minimum score to accept peer exchange
- **Opportunistic Graft Threshold** (5.0): Score needed for grafting optimization

### Topic Scoring
Topic scoring evaluates peer behavior within specific gossipsub topics using four parameters:
- **P1 (Time in Mesh)**: Rewards peers for staying connected to mesh networks
- **P2 (First Message Deliveries)**: Rewards first delivery of valid messages
- **P3 (Mesh Message Deliveries)**: Evaluates message delivery within mesh (disabled in SSV)
- **P4 (Invalid Message Deliveries)**: Penalizes delivery of invalid messages

### Decay Mechanisms
All scores use exponential decay to reduce influence of old behavior:
- **Decay Factor**: Calculated to reach 1% of original value over specified lifetime
- **Decay Convergence**: Steady-state value under constant behavior patterns
- **Decay Threshold**: Point where decay reaches target value

## Key Features

### Dynamic Configuration
- Adapts to network topology (subnet count, validator distribution)
- Configurable message rates and expected behavior patterns
- Runtime parameter adjustment for changing network conditions

### SSV Compatibility
- Parameters match Go reference implementation for network consistency
- Ethereum 2.0 epoch-based timing (32 slots × 12 seconds by default)
- Subnet-aware scoring with equal weight distribution

### Robustness
- NaN/Infinity sanitization prevents mathematical instability
- Comprehensive error handling for edge cases
- Extensive test coverage for boundary conditions

## Implementation Details

### Peer Score Parameters
```rust
pub fn peer_score_params(one_epoch: Duration) -> gossipsub::PeerScoreParams
```
Configures peer scoring with:
- Topic score cap: 32.72
- Decay interval: One epoch duration
- Behavior penalty calculation based on expected violation rates
- IP colocation protection with threshold of 10 peers per IP

### Topic Score Parameters
```rust
pub fn topic_score_params_for_subnet_with_rate<E: EthSpec>(
    subnet: SubnetId,
    subnet_count: usize, 
    message_rate: f64,
    chain_spec: &ChainSpec,
) -> TopicScoreParams
```
Generates topic-specific parameters:
- Weight distribution across subnets: `TOTAL_TOPICS_WEIGHT / subnet_count`
- Message rate adaptation for different network loads
- Activation delays for mesh scoring (3 epochs)

### Utility Functions
- `calculate_score_decay_factor()`: Computes decay multiplier for given lifetime
- `decay_convergence()`: Calculates steady-state score under constant rates
- `decay_threshold()`: Determines threshold where decay reaches target

## Configuration Options

### Network Configuration
```rust
pub struct NetworkConfig {
    pub subnets: usize,                    // Number of subnets
    pub one_epoch_duration: Duration,      // Epoch timing
    pub total_topics_weight: f64,          // Weight allocation
}
```

### Topic Configuration
```rust
pub struct TopicConfig {
    pub d: usize,                          // Gossip degree
    pub expected_msg_rate: f64,            // Expected message rate
    pub topic_weight: f64,                 // Topic importance weight
    // ... timing and threshold parameters
}
```

## Error Handling

The component provides comprehensive error handling:
- Invalid decay factors (≥ 1.0) are rejected with descriptive errors
- NaN/Infinity values are sanitized with sensible defaults
- Failed parameter generation falls back to safe defaults with warnings

## Performance Considerations

- Decay calculations use efficient exponential operations
- Parameter sanitization runs only when needed
- Memory-efficient storage of configuration structs
- Minimal runtime overhead for score updates

## Integration Points

### Dependencies
- `gossipsub`: Core gossipsub protocol implementation
- `subnet_service`: Subnet management and identification
- `types`: Ethereum 2.0 type definitions and chain specifications
- `tracing`: Structured logging for debugging and monitoring

### Usage Context
This component is used by:
- Gossipsub network layer for peer reputation management
- Subnet services for topic-specific behavior evaluation
- Network diagnostics for peer quality assessment

## Testing

The component includes comprehensive tests for:
- Boundary conditions (zero, infinity, NaN values)
- Mathematical correctness of decay calculations
- Parameter sanitization effectiveness
- Integration with gossipsub types
- SSV specification compliance

## Future Considerations

- Dynamic adjustment of scoring parameters based on network conditions
- Machine learning integration for adaptive threshold tuning
- Cross-subnet reputation sharing mechanisms
- Performance optimization for large-scale deployments

This scoring system ensures network health by incentivizing good behavior and penalizing malicious or unreliable peers while maintaining compatibility with SSV network specifications.