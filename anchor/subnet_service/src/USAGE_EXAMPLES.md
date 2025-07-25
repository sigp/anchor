# Subnet Service Usage Examples

## Basic Setup and Initialization

### Starting the Subnet Service

```rust
use subnet_service::{start_subnet_service, SubnetEvent};
use database::NetworkState;
use task_executor::TaskExecutor;
use slot_clock::SystemTimeSlotClock;
use types::{ChainSpec, MainnetEthSpec};
use tokio::sync::watch;
use std::sync::Arc;

async fn initialize_subnet_service() {
    // Create database watch channel
    let (db_tx, db_rx) = watch::channel(NetworkState::default());
    
    // Setup task executor
    let executor = TaskExecutor::new();
    
    // Create slot clock
    let slot_clock = SystemTimeSlotClock::new(
        types::Slot::new(0),
        std::time::Duration::from_secs(0),
        std::time::Duration::from_secs(12), // 12-second slots
    );
    
    // Chain specification
    let chain_spec = Arc::new(ChainSpec::mainnet());
    
    // Start subnet service
    let mut subnet_events = start_subnet_service::<MainnetEthSpec>(
        db_rx,
        128,    // subnet_count
        false,  // subscribe_all_subnets
        false,  // disable_gossipsub_topic_scoring
        &executor,
        slot_clock,
        chain_spec,
    );
    
    // Handle subnet events
    while let Some(event) = subnet_events.recv().await {
        match event {
            SubnetEvent::Join(subnet_id, message_rate) => {
                println!("Joining subnet {} with rate {:?}", *subnet_id, message_rate);
                // Subscribe to gossipsub topic for this subnet
            }
            SubnetEvent::Leave(subnet_id) => {
                println!("Leaving subnet {}", *subnet_id);
                // Unsubscribe from gossipsub topic
            }
            SubnetEvent::RateUpdate(subnet_id, new_rate) => {
                println!("Rate update for subnet {}: {}", *subnet_id, new_rate);
                // Update gossipsub scoring parameters
            }
        }
    }
}
```

## Configuration Variants

### Subscribe to All Subnets Mode

```rust
async fn full_network_monitoring() {
    let (db_tx, db_rx) = watch::channel(NetworkState::default());
    let executor = TaskExecutor::new();
    let slot_clock = SystemTimeSlotClock::new(
        types::Slot::new(0),
        std::time::Duration::from_secs(0),
        std::time::Duration::from_secs(12),
    );
    let chain_spec = Arc::new(ChainSpec::mainnet());
    
    // Subscribe to ALL subnets initially
    let mut subnet_events = start_subnet_service::<MainnetEthSpec>(
        db_rx,
        128,   // subnet_count  
        true,  // subscribe_all_subnets = true
        false, // enable scoring
        &executor,
        slot_clock,
        chain_spec,
    );
    
    // Will receive Join events for all 128 subnets immediately
    let mut join_count = 0;
    while let Some(event) = subnet_events.recv().await {
        match event {
            SubnetEvent::Join(subnet_id, message_rate) => {
                join_count += 1;
                println!("Joined subnet {} ({}/128), rate: {:?}", 
                         *subnet_id, join_count, message_rate);
                
                if join_count == 128 {
                    println!("All subnets joined!");
                    break;
                }
            }
            _ => {}
        }
    }
}
```

### Disable Scoring Mode

```rust
async fn simple_subnet_tracking() {
    let (db_tx, db_rx) = watch::channel(NetworkState::default());
    let executor = TaskExecutor::new();
    let slot_clock = SystemTimeSlotClock::new(
        types::Slot::new(0),
        std::time::Duration::from_secs(0),
        std::time::Duration::from_secs(12),
    );
    let chain_spec = Arc::new(ChainSpec::mainnet());
    
    // Disable message rate calculations
    let mut subnet_events = start_subnet_service::<MainnetEthSpec>(
        db_rx,
        128,
        false,
        true,  // disable_gossipsub_topic_scoring = true
        &executor,
        slot_clock,
        chain_spec,
    );
    
    while let Some(event) = subnet_events.recv().await {
        match event {
            SubnetEvent::Join(subnet_id, message_rate) => {
                // message_rate will always be None when scoring is disabled
                assert!(message_rate.is_none());
                println!("Joined subnet {} (no scoring)", *subnet_id);
            }
            SubnetEvent::Leave(subnet_id) => {
                println!("Left subnet {}", *subnet_id);
            }
            SubnetEvent::RateUpdate(_, _) => {
                // This event will never occur when scoring is disabled
                unreachable!("Rate updates disabled");
            }
        }
    }
}
```

## Working with SubnetId

### Creating and Converting SubnetIds

```rust
use subnet_service::{SubnetId, SUBNET_COUNT};
use ssv_types::CommitteeId;

fn subnet_id_examples() {
    // Create from raw u64
    let subnet1 = SubnetId::new(42);
    let subnet2 = SubnetId::from(25u64);
    
    // Access underlying u64 value
    let raw_id: u64 = *subnet1;
    println!("Subnet ID: {}", raw_id);
    
    // Derive from committee ID
    let committee_id = CommitteeId::from([1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16,
                                         17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32]);
    let subnet_from_committee = SubnetId::from_committee(committee_id, SUBNET_COUNT);
    
    println!("Committee {} maps to subnet {}", 
             hex::encode(committee_id), *subnet_from_committee);
    
    // SubnetIds can be used in collections
    use std::collections::HashSet;
    let mut active_subnets = HashSet::new();
    active_subnets.insert(subnet1);
    active_subnets.insert(subnet2);
    
    println!("Managing {} active subnets", active_subnets.len());
}
```

## Message Rate Calculations

### Manual Message Rate Calculation

```rust
use subnet_service::{calculate_message_rate_for_subnet, get_committee_info_for_subnet};
use database::NetworkState;
use types::{ChainSpec, MainnetEthSpec};
use std::sync::Arc;

fn calculate_rates_manually() {
    // Create mock network state
    let network_state = NetworkState::default();
    let chain_spec = ChainSpec::mainnet();
    
    // Calculate rate for specific subnet
    let subnet_id = SubnetId::new(10);
    let message_rate = calculate_message_rate_for_subnet::<MainnetEthSpec>(
        &subnet_id,
        &network_state,
        &chain_spec,
    );
    
    println!("Subnet {} message rate: {:.2} msgs/sec", *subnet_id, message_rate);
    
    // Get committee info for debugging
    let committee_info = get_committee_info_for_subnet(&subnet_id, &network_state);
    println!("Subnet {} has {} committees", *subnet_id, committee_info.len());
    
    for (i, committee) in committee_info.iter().enumerate() {
        println!("  Committee {}: {} operators, {} validators",
                 i, committee.committee_members.len(), committee.validator_indices.len());
    }
}
```

### Using Message Rate Components

```rust
use subnet_service::message_rate::{MessageCounts, calculate_message_rate_for_topic};
use ssv_types::{CommitteeInfo, IndexSet, OperatorId, ValidatorIndex};
use types::{ChainSpec, MainnetEthSpec};

fn message_rate_breakdown() {
    let chain_spec = ChainSpec::mainnet();
    
    // Create sample committee configuration
    let mut committee_members = IndexSet::new();
    committee_members.insert(OperatorId(1));
    committee_members.insert(OperatorId(2));
    committee_members.insert(OperatorId(3));
    committee_members.insert(OperatorId(4));
    
    let validator_indices = vec![
        ValidatorIndex(100),
        ValidatorIndex(101),
        ValidatorIndex(102),
    ];
    
    let committee_info = CommitteeInfo {
        committee_members,
        validator_indices,
    };
    
    // Calculate message rate
    let rate = calculate_message_rate_for_topic::<MainnetEthSpec>(
        &[committee_info.clone()],
        &chain_spec,
    );
    
    println!("Committee message rate: {:.4} msgs/sec", rate);
    
    // Examine message count structure
    let committee_size = committee_info.committee_members.len();
    
    let with_pre = MessageCounts::duty_with_pre_consensus(committee_size);
    let without_pre = MessageCounts::duty_without_pre_consensus(committee_size);
    
    println!("Message counts for {} operators:", committee_size);
    println!("  With pre-consensus: {} pre + {} consensus + {} post = {} total",
             with_pre.pre_consensus, with_pre.consensus, 
             with_pre.post_consensus, with_pre.total());
    println!("  Without pre-consensus: {} pre + {} consensus + {} post = {} total",
             without_pre.pre_consensus, without_pre.consensus,
             without_pre.post_consensus, without_pre.total());
}
```

## Testing and Development

### Mock Subnet Service for Testing

```rust
use subnet_service::{test_tracker, SubnetEvent, SubnetId};
use task_executor::TaskExecutor;
use std::time::Duration;

async fn test_subnet_events() {
    let executor = TaskExecutor::new();
    
    // Create test events
    let test_events = vec![
        SubnetEvent::Join(SubnetId::new(1), Some(5.5)),
        SubnetEvent::Join(SubnetId::new(2), Some(3.2)),
        SubnetEvent::RateUpdate(SubnetId::new(1), 6.1),
        SubnetEvent::Leave(SubnetId::new(2)),
    ];
    
    // Create mock tracker with 100ms delays between events
    let mut event_rx = test_tracker(
        executor,
        test_events,
        Duration::from_millis(100),
    );
    
    // Process test events
    while let Some(event) = event_rx.recv().await {
        match event {
            SubnetEvent::Join(subnet_id, rate) => {
                println!("Test: Joined subnet {} with rate {:?}", *subnet_id, rate);
            }
            SubnetEvent::Leave(subnet_id) => {
                println!("Test: Left subnet {}", *subnet_id);
            }
            SubnetEvent::RateUpdate(subnet_id, rate) => {
                println!("Test: Updated subnet {} rate to {}", *subnet_id, rate);
            }
        }
    }
    
    println!("Test completed");
}
```

### Integration with Gossipsub

```rust
use subnet_service::{SubnetEvent, SubnetId};
use libp2p::gossipsub::{Gossipsub, IdentTopic, TopicHash};

struct NetworkManager {
    gossipsub: Gossipsub,
    active_subnets: std::collections::HashSet<SubnetId>,
}

impl NetworkManager {
    async fn handle_subnet_event(&mut self, event: SubnetEvent) -> Result<(), Box<dyn std::error::Error>> {
        match event {
            SubnetEvent::Join(subnet_id, message_rate) => {
                // Create topic for subnet
                let topic_name = format!("ssv_subnet_{}", *subnet_id);
                let topic = IdentTopic::new(topic_name);
                
                // Subscribe to gossipsub topic
                self.gossipsub.subscribe(&topic)?;
                self.active_subnets.insert(subnet_id);
                
                println!("Subscribed to subnet {} topic", *subnet_id);
                
                // Configure scoring if rate provided
                if let Some(rate) = message_rate {
                    self.configure_topic_scoring(&topic.hash(), rate);
                }
            }
            
            SubnetEvent::Leave(subnet_id) => {
                let topic_name = format!("ssv_subnet_{}", *subnet_id);
                let topic = IdentTopic::new(topic_name);
                
                // Unsubscribe from gossipsub topic
                self.gossipsub.unsubscribe(&topic)?;
                self.active_subnets.remove(&subnet_id);
                
                println!("Unsubscribed from subnet {} topic", *subnet_id);
            }
            
            SubnetEvent::RateUpdate(subnet_id, new_rate) => {
                let topic_name = format!("ssv_subnet_{}", *subnet_id);
                let topic = IdentTopic::new(topic_name);
                
                // Update existing topic scoring
                self.configure_topic_scoring(&topic.hash(), new_rate);
                
                println!("Updated scoring for subnet {} to rate {}", *subnet_id, new_rate);
            }
        }
        
        Ok(())
    }
    
    fn configure_topic_scoring(&mut self, topic_hash: &TopicHash, message_rate: f64) {
        // Configure gossipsub topic scoring parameters based on expected message rate
        // This would integrate with libp2p-gossipsub's scoring mechanisms
        println!("Configuring topic scoring: rate = {:.2} msgs/sec", message_rate);
    }
}
```

## Error Handling Patterns

### Robust Event Processing

```rust
use subnet_service::{start_subnet_service, SubnetEvent};
use tracing::{error, warn, info};

async fn robust_subnet_handling() {
    let mut subnet_events = setup_subnet_service().await; // Your setup function
    
    loop {
        match subnet_events.recv().await {
            Some(event) => {
                if let Err(e) = process_subnet_event(event).await {
                    error!("Failed to process subnet event: {}", e);
                    // Continue processing other events
                }
            }
            None => {
                warn!("Subnet service channel closed, reconnecting...");
                // Restart subnet service
                subnet_events = setup_subnet_service().await;
            }
        }
    }
}

async fn process_subnet_event(event: SubnetEvent) -> Result<(), Box<dyn std::error::Error>> {
    match event {
        SubnetEvent::Join(subnet_id, rate) => {
            info!("Processing join for subnet {}", *subnet_id);
            // Your join logic here
            join_subnet(subnet_id, rate).await?;
        }
        SubnetEvent::Leave(subnet_id) => {
            info!("Processing leave for subnet {}", *subnet_id);
            // Your leave logic here  
            leave_subnet(subnet_id).await?;
        }
        SubnetEvent::RateUpdate(subnet_id, rate) => {
            info!("Processing rate update for subnet {}: {}", *subnet_id, rate);
            // Your rate update logic here
            update_subnet_rate(subnet_id, rate).await?;
        }
    }
    Ok(())
}

async fn join_subnet(subnet_id: SubnetId, rate: Option<f64>) -> Result<(), Box<dyn std::error::Error>> {
    // Implementation
    Ok(())
}

async fn leave_subnet(subnet_id: SubnetId) -> Result<(), Box<dyn std::error::Error>> {
    // Implementation  
    Ok(())
}

async fn update_subnet_rate(subnet_id: SubnetId, rate: f64) -> Result<(), Box<dyn std::error::Error>> {
    // Implementation
    Ok(())
}

async fn setup_subnet_service() -> tokio::sync::mpsc::Receiver<SubnetEvent> {
    // Your subnet service setup logic
    todo!()
}
```

## Performance Monitoring

### Tracking Subnet Activity

```rust
use subnet_service::{SubnetEvent, SubnetId};
use std::collections::HashMap;
use std::time::Instant;

struct SubnetMetrics {
    join_time: Instant,
    message_count: u64,
    current_rate: Option<f64>,
}

struct SubnetMonitor {
    metrics: HashMap<SubnetId, SubnetMetrics>,
    total_joins: u64,
    total_leaves: u64,
}

impl SubnetMonitor {
    fn new() -> Self {
        Self {
            metrics: HashMap::new(),
            total_joins: 0,
            total_leaves: 0,
        }
    }
    
    fn handle_event(&mut self, event: &SubnetEvent) {
        match event {
            SubnetEvent::Join(subnet_id, rate) => {
                self.total_joins += 1;
                self.metrics.insert(*subnet_id, SubnetMetrics {
                    join_time: Instant::now(),
                    message_count: 0,
                    current_rate: *rate,
                });
                println!("Subnet {} joined (total active: {})", 
                         *subnet_id, self.metrics.len());
            }
            
            SubnetEvent::Leave(subnet_id) => {
                self.total_leaves += 1;
                if let Some(metrics) = self.metrics.remove(subnet_id) {
                    let duration = metrics.join_time.elapsed();
                    println!("Subnet {} left after {:?} (messages: {})", 
                             *subnet_id, duration, metrics.message_count);
                }
            }
            
            SubnetEvent::RateUpdate(subnet_id, new_rate) => {
                if let Some(metrics) = self.metrics.get_mut(subnet_id) {
                    println!("Subnet {} rate: {:?} -> {}", 
                             *subnet_id, metrics.current_rate, new_rate);
                    metrics.current_rate = Some(*new_rate);
                }
            }
        }
    }
    
    fn print_stats(&self) {
        println!("Subnet Statistics:");
        println!("  Active subnets: {}", self.metrics.len());
        println!("  Total joins: {}", self.total_joins);
        println!("  Total leaves: {}", self.total_leaves);
        
        let total_rate: f64 = self.metrics.values()
            .filter_map(|m| m.current_rate)
            .sum();
        println!("  Total message rate: {:.2} msgs/sec", total_rate);
    }
}