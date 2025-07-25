# Scoring Component Technical Specification

## Module Structure

### Files
- `mod.rs` - Module exports and shared utility functions
- `peer_score_config.rs` - Peer-level scoring configuration
- `topic_score_config.rs` - Topic-specific scoring configuration

### Dependencies
```rust
use std::time::Duration;
use gossipsub::{PeerScoreParams, PeerScoreThresholds, TopicScoreParams};
use subnet_service::SubnetId;
use tracing::{debug, warn};
use types::{ChainSpec, EthSpec};
```

## Constants

### Peer Scoring Thresholds
```rust
pub const GOSSIP_THRESHOLD: f64 = -4000.0;        // Minimum score for gossip participation
pub const PUBLISH_THRESHOLD: f64 = -8000.0;       // Minimum score for message publishing
pub const GRAYLIST_THRESHOLD: f64 = -16000.0;     // Score below which peers are rejected
pub const ACCEPT_PX_THRESHOLD: f64 = 100.0;       // Minimum score for peer exchange
pub const OPPORTUNISTIC_GRAFT_THRESHOLD: f64 = 5.0; // Score for grafting optimization
```

### Peer Scoring Parameters
```rust
pub const TOPIC_SCORE_CAP: f64 = 32.72;                    // Maximum topic score contribution
pub const DECAY_TO_ZERO: f64 = 0.01;                       // Target decay convergence (1%)
pub const RETAIN_SCORE_EPOCH_MULTIPLIER: u32 = 100;        // Score retention duration
pub const APP_SPECIFIC_WEIGHT: f64 = 0.0;                  // Application-specific scoring weight
pub const IP_COLOCATION_FACTOR_THRESHOLD: f64 = 10.0;      // Max peers per IP before penalty
pub const IP_COLOCATION_FACTOR_WEIGHT: f64 = -TOPIC_SCORE_CAP; // IP colocation penalty weight
pub const BEHAVIOUR_PENALTY_THRESHOLD: f64 = 6.0;          // Behavior penalty threshold
```

### Topic Scoring Parameters
```rust
const GOSSIPSUB_D: usize = 8;                               // Gossip degree parameter
const TOTAL_TOPICS_WEIGHT: f64 = 4.0;                      // Total weight across all topics
const MAX_TIME_IN_MESH_SCORE: f64 = 10.0;                  // Maximum P1 score
const TIME_IN_MESH_QUANTUM: Duration = Duration::from_secs(12); // P1 time quantum
const TIME_IN_MESH_QUANTUM_CAP: Duration = Duration::from_secs(3600); // P1 time cap (1 hour)
const FIRST_DELIVERY_DECAY_EPOCHS: u32 = 4;                // P2 decay period
const MAX_FIRST_DELIVERY_SCORE: f64 = 80.0;                // Maximum P2 score
const MESH_DELIVERY_DECAY_EPOCHS: u32 = 16;                // P3 decay period
const MESH_DELIVERY_DAMPENING_FACTOR: f64 = 1.0 / 50.0;    // P3 dampening factor
const MESH_DELIVERY_CAP_FACTOR: f64 = 16.0;                // P3 cap multiplier
const INVALID_MESSAGE_DECAY_EPOCHS: u32 = 100;             // P4 decay period
const MAX_INVALID_MESSAGES_ALLOWED: usize = 20;            // P4 threshold
```

## Data Structures

### NetworkConfig
```rust
#[derive(Debug, Clone)]
pub struct NetworkConfig {
    pub subnets: usize,                    // Total number of subnets in network
    pub one_epoch_duration: Duration,      // Duration of one Ethereum epoch
    pub total_topics_weight: f64,          // Total weight distributed across topics
}
```

### TopicConfig
```rust
#[derive(Debug, Clone)]
pub struct TopicConfig {
    pub d: usize,                          // Gossip degree (D parameter)
    pub expected_msg_rate: f64,            // Expected messages per second
    pub topic_weight: f64,                 // Weight assigned to this topic
    
    // P1: Time in Mesh parameters
    pub max_time_in_mesh_score: f64,
    pub time_in_mesh_quantum: Duration,
    pub time_in_mesh_quantum_cap: Duration,
    
    // P2: First Message Deliveries parameters
    pub first_delivery_decay_epochs: u32,
    pub max_first_delivery_score: f64,
    
    // P3: Mesh Message Deliveries parameters
    pub mesh_delivery_decay_epochs: u32,
    pub mesh_delivery_dampening_factor: f64,
    pub mesh_delivery_cap_factor: f64,
    pub mesh_delivery_activation_time: Duration,
    
    // P4: Invalid Message Deliveries parameters
    pub invalid_message_decay_epochs: u32,
    pub max_invalid_messages_allowed: usize,
}
```

### TopicScoringOptions
```rust
#[derive(Debug, Clone)]
pub struct TopicScoringOptions {
    pub network: NetworkConfig,
    pub topic: TopicConfig,
}
```

## Function Specifications

### Core Peer Scoring Functions

#### `peer_score_params(one_epoch: Duration) -> PeerScoreParams`
**Purpose**: Generate peer scoring parameters for gossipsub  
**Parameters**:
- `one_epoch`: Duration of one Ethereum epoch (typically 32 slots × 12 seconds)

**Returns**: Configured `PeerScoreParams` struct with:
- Topic score cap and decay settings
- Behavior penalty calculations
- IP colocation protection
- Score retention duration

**Algorithm**:
1. Calculate behavior penalty decay: `decay_factor = DECAY_TO_ZERO^(1 / (lifetime / decay_interval))`
2. Compute target convergence value: `target = decay_convergence(decay, max_rate) - threshold`
3. Calculate penalty weight: `weight = GOSSIP_THRESHOLD / (target²)`

#### `peer_score_thresholds() -> PeerScoreThresholds`
**Purpose**: Generate peer scoring thresholds  
**Returns**: Static threshold configuration matching SSV specification

### Core Topic Scoring Functions

#### `topic_score_params_for_subnet_with_rate<E: EthSpec>(...) -> TopicScoreParams`
**Purpose**: Generate topic scoring parameters for specific subnet  
**Parameters**:
- `subnet`: Subnet identifier
- `subnet_count`: Total number of subnets
- `message_rate`: Expected messages per second for this topic
- `chain_spec`: Ethereum chain specification

**Returns**: Configured `TopicScoreParams` or default on error

**Algorithm**:
1. Create network config: `epoch_duration = slots_per_epoch × seconds_per_slot`
2. Calculate topic weight: `weight = TOTAL_TOPICS_WEIGHT / subnet_count`
3. Generate parameters using `TopicScoringOptions::to_topic_score_params()`

#### `TopicScoringOptions::to_topic_score_params(&self) -> Result<TopicScoreParams, String>`
**Purpose**: Convert configuration to gossipsub parameters  
**Returns**: `TopicScoreParams` or error string

**Algorithm**:
1. **P1 Calculation**:
   ```rust
   time_in_mesh_cap = quantum_cap / quantum
   time_in_mesh_weight = max_score / cap
   ```

2. **P2 Calculation**:
   ```rust
   decay = calculate_score_decay_factor(decay_duration, interval)
   cap = decay_convergence(decay, 2.0 × msg_rate / d)
   weight = max_score / cap
   ```

3. **P3 Calculation**:
   ```rust
   threshold = decay_threshold(decay, msg_rate × dampening)
   cap = threshold × cap_factor
   weight = 0.0  // Disabled in SSV
   ```

4. **P4 Calculation**:
   ```rust
   weight = GRAYLIST_THRESHOLD / (topic_weight × max_invalid²)
   ```

### Utility Functions

#### `calculate_score_decay_factor(lifetime: Duration, decay_interval: Duration) -> f64`
**Purpose**: Calculate exponential decay factor  
**Formula**: `DECAY_TO_ZERO^(1 / (lifetime / interval))`  
**Ensures**: Score decays to 1% over specified lifetime

#### `decay_convergence(decay: f64, rate_per_interval: f64) -> Result<f64, String>`
**Purpose**: Calculate steady-state value under constant rate  
**Formula**: `rate / (1 - decay)`  
**Validation**: Returns error if `decay >= 1.0`

#### `decay_threshold(decay_factor: f64, target_value: f64) -> Result<f64, String>`
**Purpose**: Calculate threshold where decay reaches target  
**Formula**: `target / (1 - decay_factor)`  
**Validation**: Returns error if `decay_factor >= 1.0` or `target_value <= 0.0`

## Parameter Sanitization

### `TopicScoringOptions::sanitize_topic_params(params: &mut TopicScoreParams) -> usize`
**Purpose**: Replace NaN/Infinity values with safe defaults  
**Returns**: Number of parameters sanitized

**Default Values**:
```rust
const DEFAULT_DECAY: f64 = 0.001;
const DEFAULT_WEIGHT: f64 = 0.0;
const DEFAULT_CAP: f64 = 1.0;
const DEFAULT_THRESHOLD: f64 = 1.0;
const DEFAULT_INVALID_WEIGHT: f64 = -0.1;
```

**Sanitized Parameters**:
- All decay factors
- All weights and caps
- All thresholds
- Invalid message delivery parameters

## Mathematical Formulas

### Exponential Decay
```
decay_factor = DECAY_TO_ZERO^(1 / number_of_intervals)
final_value = initial_value × decay_factor^intervals
```

### Steady-State Convergence
```
steady_state = rate_per_interval / (1 - decay_factor)
```

### Behavior Penalty Weight
```
weight = GOSSIP_THRESHOLD / (convergence_value - threshold)²
```

### Topic Weight Distribution
```
topic_weight = TOTAL_TOPICS_WEIGHT / number_of_subnets
```

## Error Conditions

### Input Validation Errors
- `decay_factor >= 1.0`: Mathematical instability
- `target_value <= 0.0`: Invalid threshold calculation
- NaN/Infinity values: Numerical instability

### Runtime Errors
- Failed decay convergence calculation
- Invalid mesh delivery threshold calculation
- Parameter sanitization required

## Performance Characteristics

### Time Complexity
- Parameter generation: O(1)
- Decay calculation: O(1) using `powf()`
- Sanitization: O(1) - fixed number of parameters

### Space Complexity
- Configuration structs: O(1) - fixed size
- Parameter generation: O(1) - no dynamic allocation

### Computational Cost
- Moderate: Uses floating-point exponentiation
- Minimal: Most calculations are simple arithmetic
- Cached: Parameters generated once per configuration change

## Integration Requirements

### Gossipsub Integration
- Must provide `PeerScoreParams` and `PeerScoreThresholds`
- Must provide `TopicScoreParams` for each topic
- Parameters must be updated when network conditions change

### SSV Compatibility
- All constants must match Go reference implementation
- Epoch timing must align with Ethereum 2.0 specifications
- Subnet weight distribution must be equal across subnets

### Error Handling
- All functions must handle mathematical edge cases
- Parameter sanitization must prevent network instability
- Fallback to safe defaults when parameter generation fails