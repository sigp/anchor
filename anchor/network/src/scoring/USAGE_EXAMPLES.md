# Scoring Component Usage Examples

## Basic Peer Scoring Setup

### Creating Peer Score Parameters
```rust
use std::time::Duration;
use crate::scoring::peer_score_config::{peer_score_params, peer_score_thresholds};

// Define one epoch duration (32 slots × 12 seconds for mainnet)
let one_epoch = Duration::from_secs(32 * 12); // 384 seconds

// Generate peer scoring parameters
let peer_params = peer_score_params(one_epoch);
let peer_thresholds = peer_score_thresholds();

// Configure gossipsub with peer scoring
let gossipsub_config = gossipsub::ConfigBuilder::default()
    .peer_score_params(peer_params)
    .peer_score_thresholds(peer_thresholds)
    .build()
    .expect("Valid gossipsub config");
```

### Custom Epoch Duration
```rust
// For testnet with different slot timing
let slot_duration = Duration::from_secs(6); // 6-second slots
let slots_per_epoch = 32;
let testnet_epoch = slot_duration * slots_per_epoch;

let peer_params = peer_score_params(testnet_epoch);
```

## Topic Scoring Configuration

### Basic Topic Scoring for Single Subnet
```rust
use types::{ChainSpec, MainnetEthSpec};
use subnet_service::SubnetId;
use crate::scoring::topic_score_config::topic_score_params_for_subnet_with_rate;

// Configure for mainnet with 64 subnets
let subnet_id = SubnetId::new(0);
let subnet_count = 64;
let expected_message_rate = 0.5; // 0.5 messages per second
let chain_spec = ChainSpec::mainnet();

let topic_params = topic_score_params_for_subnet_with_rate::<MainnetEthSpec>(
    subnet_id,
    subnet_count,
    expected_message_rate,
    &chain_spec,
);

// Apply to gossipsub topic
let topic = gossipsub::IdentTopic::new("some_topic");
gossipsub_instance.set_topic_params(&topic.hash(), topic_params);
```

### Multiple Subnet Configuration
```rust
use std::collections::HashMap;

fn configure_all_subnets<E: EthSpec>(
    subnet_count: usize,
    base_message_rate: f64,
    chain_spec: &ChainSpec,
) -> HashMap<SubnetId, TopicScoreParams> {
    let mut subnet_configs = HashMap::new();
    
    for subnet_num in 0..subnet_count {
        let subnet_id = SubnetId::new(subnet_num as u64);
        
        // Vary message rate based on subnet activity
        let message_rate = match subnet_num {
            0..=15 => base_message_rate * 1.5,  // High-activity subnets
            16..=47 => base_message_rate,       // Normal activity
            _ => base_message_rate * 0.7,       // Lower activity
        };
        
        let params = topic_score_params_for_subnet_with_rate::<E>(
            subnet_id,
            subnet_count,
            message_rate,
            chain_spec,
        );
        
        subnet_configs.insert(subnet_id, params);
    }
    
    subnet_configs
}

// Usage
let subnet_configs = configure_all_subnets::<MainnetEthSpec>(
    64,
    0.5,
    &ChainSpec::mainnet(),
);
```

## Advanced Topic Scoring Options

### Custom Topic Configuration
```rust
use crate::scoring::topic_score_config::{
    NetworkConfig, TopicConfig, TopicScoringOptions
};

// Create custom network configuration
let network_config = NetworkConfig {
    subnets: 32,                                    // Smaller network
    one_epoch_duration: Duration::from_secs(192),   // Faster epochs
    total_topics_weight: 8.0,                       // Higher total weight
};

// Create custom topic configuration
let topic_config = TopicConfig {
    d: 6,                                           // Lower gossip degree
    expected_msg_rate: 1.0,                         // Higher message rate
    topic_weight: network_config.total_topics_weight / 32.0,
    max_time_in_mesh_score: 15.0,                   // Higher P1 reward
    first_delivery_decay_epochs: 2,                 // Faster P2 decay
    max_first_delivery_score: 100.0,                // Higher P2 reward
    mesh_delivery_activation_time: Duration::from_secs(576), // 3 epochs
    ..Default::default()
};

// Combine into scoring options
let scoring_options = TopicScoringOptions {
    network: network_config,
    topic: topic_config,
};

// Generate parameters
match scoring_options.to_topic_score_params() {
    Ok(params) => {
        println!("Generated custom topic parameters");
        // Use params...
    },
    Err(e) => {
        eprintln!("Failed to generate parameters: {}", e);
        // Handle error...
    }
}
```

### Dynamic Parameter Adjustment
```rust
use tracing::info;

fn adjust_scoring_for_network_conditions(
    base_options: &TopicScoringOptions,
    network_load: f64,  // 0.0 to 2.0 multiplier
) -> Result<TopicScoreParams, String> {
    let mut adjusted_options = base_options.clone();
    
    // Adjust message rate based on network load
    adjusted_options.topic.expected_msg_rate *= network_load;
    
    // Adjust scoring thresholds for high load
    if network_load > 1.5 {
        adjusted_options.topic.max_invalid_messages_allowed = 10; // Stricter
        adjusted_options.topic.first_delivery_decay_epochs = 2;   // Faster decay
        info!("Applied high-load scoring adjustments");
    } else if network_load < 0.5 {
        adjusted_options.topic.max_invalid_messages_allowed = 30; // More lenient
        adjusted_options.topic.first_delivery_decay_epochs = 8;   // Slower decay
        info!("Applied low-load scoring adjustments");
    }
    
    adjusted_options.to_topic_score_params()
}

// Usage
let base_options = TopicScoringOptions::new_with_rate::<MainnetEthSpec>(
    64, 0.5, &ChainSpec::mainnet()
);

let current_load = 1.8; // High network load
match adjust_scoring_for_network_conditions(&base_options, current_load) {
    Ok(params) => {
        // Apply adjusted parameters
    },
    Err(e) => {
        eprintln!("Parameter adjustment failed: {}", e);
    }
}
```

## Utility Function Usage

### Decay Calculations
```rust
use crate::scoring::peer_score_config::{
    calculate_score_decay_factor, decay_convergence
};

// Calculate how fast scores should decay
let penalty_lifetime = Duration::from_secs(3600); // 1 hour
let decay_interval = Duration::from_secs(384);    // 1 epoch
let decay_factor = calculate_score_decay_factor(penalty_lifetime, decay_interval);

println!("Decay factor: {:.6}", decay_factor);
// Output: Decay factor: 0.630957 (approximately)

// Calculate steady-state score under constant bad behavior
let violations_per_epoch = 5.0;
match decay_convergence(decay_factor, violations_per_epoch) {
    Ok(steady_state) => {
        println!("Steady-state penalty: {:.2}", steady_state);
        // Output: Steady-state penalty: 13.55 (approximately)
    },
    Err(e) => eprintln!("Convergence calculation failed: {}", e),
}
```

### Threshold Calculations
```rust
use crate::scoring::decay_threshold;

// Calculate threshold for mesh delivery requirements
let mesh_decay = 0.95;
let required_delivery_rate = 0.8; // 80% of expected messages

match decay_threshold(mesh_decay, required_delivery_rate) {
    Ok(threshold) => {
        println!("Mesh delivery threshold: {:.2}", threshold);
        // Output: Mesh delivery threshold: 16.00
    },
    Err(e) => eprintln!("Threshold calculation failed: {}", e),
}
```

## Error Handling Patterns

### Robust Parameter Generation
```rust
use tracing::{warn, error};

fn generate_topic_params_with_fallback<E: EthSpec>(
    subnet: SubnetId,
    subnet_count: usize,
    message_rate: f64,
    chain_spec: &ChainSpec,
) -> TopicScoreParams {
    // Try with provided parameters
    let params = topic_score_params_for_subnet_with_rate::<E>(
        subnet, subnet_count, message_rate, chain_spec
    );
    
    // Check if parameters are valid (not all defaults)
    if params.topic_weight > 0.0 {
        return params;
    }
    
    warn!(
        subnet = *subnet,
        "Generated parameters appear invalid, trying with default rate"
    );
    
    // Fallback to default message rate
    let fallback_params = topic_score_params_for_subnet_with_rate::<E>(
        subnet, subnet_count, 0.1, chain_spec  // Conservative rate
    );
    
    if fallback_params.topic_weight > 0.0 {
        return fallback_params;
    }
    
    error!(subnet = *subnet, "All parameter generation attempts failed");
    TopicScoreParams::default()
}
```

### Validation Helpers
```rust
fn validate_peer_score_params(params: &gossipsub::PeerScoreParams) -> Result<(), String> {
    if params.topic_score_cap <= 0.0 {
        return Err("Topic score cap must be positive".to_string());
    }
    
    if params.decay_interval.is_zero() {
        return Err("Decay interval cannot be zero".to_string());
    }
    
    if params.behaviour_penalty_threshold < 0.0 {
        return Err("Behavior penalty threshold cannot be negative".to_string());
    }
    
    Ok(())
}

fn validate_topic_score_params(params: &TopicScoreParams) -> Result<(), String> {
    if params.topic_weight.is_nan() || params.topic_weight.is_infinite() {
        return Err("Topic weight contains invalid value".to_string());
    }
    
    if params.first_message_deliveries_decay >= 1.0 {
        return Err("First message deliveries decay must be < 1.0".to_string());
    }
    
    Ok(())
}

// Usage
let peer_params = peer_score_params(Duration::from_secs(384));
if let Err(e) = validate_peer_score_params(&peer_params) {
    eprintln!("Invalid peer parameters: {}", e);
}
```

## Integration Examples

### Gossipsub Integration
```rust
use libp2p::gossipsub::{Gossipsub, GossipsubEvent};
use libp2p::swarm::SwarmEvent;

async fn setup_scored_gossipsub<E: EthSpec>(
    local_key: &libp2p::identity::Keypair,
    chain_spec: &ChainSpec,
) -> Result<Gossipsub, Box<dyn std::error::Error>> {
    let one_epoch = Duration::from_secs(
        E::slots_per_epoch() as u64 * chain_spec.seconds_per_slot
    );
    
    // Configure peer scoring
    let peer_params = peer_score_params(one_epoch);
    let peer_thresholds = peer_score_thresholds();
    
    // Build gossipsub
    let config = gossipsub::ConfigBuilder::default()
        .heartbeat_interval(Duration::from_secs(1))
        .validation_mode(gossipsub::ValidationMode::Strict)
        .peer_score_params(peer_params)
        .peer_score_thresholds(peer_thresholds)
        .build()?;
    
    let mut gossipsub = Gossipsub::new(
        gossipsub::MessageAuthenticity::Signed(local_key.clone()),
        config,
    )?;
    
    // Configure topic scoring for each subnet
    for subnet_num in 0..64 {
        let subnet_id = SubnetId::new(subnet_num);
        let topic = format!("subnet_{}", subnet_num);
        let topic_hash = gossipsub::IdentTopic::new(topic).hash();
        
        let topic_params = topic_score_params_for_subnet_with_rate::<E>(
            subnet_id, 64, 0.5, chain_spec
        );
        
        gossipsub.set_topic_params(&topic_hash, topic_params);
        gossipsub.subscribe(&gossipsub::IdentTopic::new(format!("subnet_{}", subnet_num)))?;
    }
    
    Ok(gossipsub)
}
```

### Monitoring and Diagnostics
```rust
use std::collections::HashMap;
use libp2p::PeerId;

struct ScoringMonitor {
    peer_scores: HashMap<PeerId, f64>,
    score_history: Vec<(std::time::Instant, PeerId, f64)>,
}

impl ScoringMonitor {
    fn new() -> Self {
        Self {
            peer_scores: HashMap::new(),
            score_history: Vec::new(),
        }
    }
    
    fn update_peer_score(&mut self, peer: PeerId, score: f64) {
        self.peer_scores.insert(peer, score);
        self.score_history.push((std::time::Instant::now(), peer, score));
        
        // Log significant score changes
        match score {
            s if s < GRAYLIST_THRESHOLD => {
                warn!(peer = %peer, score = s, "Peer graylisted due to low score");
            },
            s if s < PUBLISH_THRESHOLD => {
                warn!(peer = %peer, score = s, "Peer restricted from publishing");
            },
            s if s < GOSSIP_THRESHOLD => {
                info!(peer = %peer, score = s, "Peer restricted from gossip");
            },
            s if s > OPPORTUNISTIC_GRAFT_THRESHOLD => {
                debug!(peer = %peer, score = s, "Peer eligible for opportunistic grafting");
            },
            _ => {},
        }
    }
    
    fn get_score_distribution(&self) -> (f64, f64, f64) {
        let scores: Vec<f64> = self.peer_scores.values().copied().collect();
        if scores.is_empty() {
            return (0.0, 0.0, 0.0);
        }
        
        let sum: f64 = scores.iter().sum();
        let mean = sum / scores.len() as f64;
        
        let min = scores.iter().fold(f64::INFINITY, |a, &b| a.min(b));
        let max = scores.iter().fold(f64::NEG_INFINITY, |a, &b| a.max(b));
        
        (mean, min, max)
    }
}

// Usage in event loop
fn handle_gossipsub_event(
    event: GossipsubEvent,
    monitor: &mut ScoringMonitor,
) {
    match event {
        GossipsubEvent::PeerScoreUpdate { peer_id, score } => {
            monitor.update_peer_score(peer_id, score);
        },
        _ => {}, // Handle other events
    }
}
```

This comprehensive set of examples demonstrates practical usage patterns for the scoring component, covering basic setup, advanced configuration, error handling, and integration with the broader gossipsub system.