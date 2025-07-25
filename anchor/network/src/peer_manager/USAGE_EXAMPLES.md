# Peer Manager Usage Examples

## Basic Setup and Initialization

### Creating a PeerManager Instance
```rust
use std::time::Duration;
use anchor::network::{Config, peer_manager::PeerManager};

// Create network configuration
let config = Config {
    target_peers: 50,
    // ... other config fields
    ..Default::default()
};

// Define epoch duration (typically from chain spec)
let one_epoch_duration = Duration::from_secs(384); // 32 slots × 12 seconds

// Create peer manager
let mut peer_manager = PeerManager::new(&config, one_epoch_duration);
```

### Integration with libp2p Swarm
```rust
use libp2p::Swarm;
use anchor::network::peer_manager::PeerManager;

// Add PeerManager to your network behaviour
#[derive(NetworkBehaviour)]
struct NetworkBehaviour {
    peer_manager: PeerManager,
    // ... other behaviours
}

// Create swarm with the behaviour
let behaviour = NetworkBehaviour {
    peer_manager: PeerManager::new(&config, one_epoch_duration),
};
let mut swarm = Swarm::new(transport, behaviour, local_peer_id);
```

## Peer Discovery and Connection Management

### Processing Discovered Peers
```rust
use discv5::enr::Enr;
use libp2p::swarm::dial_opts::DialOpts;

// When discovery finds a new peer
let discovered_enr: Enr = /* from discovery */;

// Process the discovered peer
if let Some(dial_opts) = peer_manager.report_discovered_peer(discovered_enr) {
    // Peer manager wants us to dial this peer
    if let Err(e) = swarm.dial(dial_opts) {
        println!("Failed to dial peer: {}", e);
    }
}
```

### Handling Discovery Results
```rust
// In your main event loop
loop {
    match swarm.select_next_some().await {
        SwarmEvent::Behaviour(NetworkBehaviourEvent::PeerManager(event)) => {
            match event {
                peer_manager::Event::PeerStore(store_event) => {
                    // Handle peer store updates
                    println!("Peer store event: {:?}", store_event);
                }
                peer_manager::Event::Heartbeat(heartbeat_event) => {
                    // Handle heartbeat actions
                    if let Some(actions) = heartbeat_event.connect_actions {
                        // Execute dial actions
                        for dial_opts in actions.dial {
                            swarm.dial(dial_opts);
                        }
                        
                        // Execute discovery actions
                        for subnet in actions.discover {
                            // Start discovery for subnet
                            start_subnet_discovery(subnet);
                        }
                    }
                    
                    if heartbeat_event.check_peer_scores {
                        // Trigger peer scoring system
                        check_and_update_peer_scores();
                    }
                }
            }
        }
        // Handle other swarm events...
    }
}
```

## Subnet Management

### Joining Subnets for Validator Duties
```rust
use subnet_service::SubnetId;

// When validator starts duties on a subnet
let subnet_id = SubnetId::new(42);
let connect_actions = peer_manager.join_subnet(subnet_id);

// Execute the returned actions
for dial_opts in connect_actions.dial {
    swarm.dial(dial_opts);
}

for subnet in connect_actions.discover {
    // Start discovery query for this subnet
    discovery_service.start_subnet_query(subnet);
}
```

### Multiple Subnet Management
```rust
// Validator duties across multiple subnets
let validator_subnets = vec![
    SubnetId::new(1),
    SubnetId::new(15),
    SubnetId::new(33),
    SubnetId::new(58),
];

for subnet_id in validator_subnets {
    let actions = peer_manager.join_subnet(subnet_id);
    
    // Process dial actions
    for dial_opts in actions.dial {
        if let Err(e) = swarm.dial(dial_opts) {
            println!("Failed to dial peer for subnet {}: {}", subnet_id, e);
        }
    }
    
    // Process discovery actions
    for discover_subnet in actions.discover {
        println!("Starting discovery for subnet: {}", discover_subnet);
        discovery_service.discover_subnet_peers(discover_subnet);
    }
}
```

## Peer Blocking and Management

### Blocking Misbehaving Peers
```rust
use discv5::libp2p_identity::PeerId;

// When peer scoring detects misbehaviour
let misbehaving_peer: PeerId = /* from scoring system */;

if peer_manager.block_peer(misbehaving_peer) {
    println!("Successfully blocked peer: {}", misbehaving_peer);
    
    // Optionally close existing connections
    swarm.disconnect_peer_id(misbehaving_peer);
} else {
    println!("Peer was already blocked: {}", misbehaving_peer);
}
```

### Manual Peer Unblocking
```rust
// Manually unblock a peer (e.g., for testing or admin action)
let peer_to_unblock: PeerId = /* peer id */;

if peer_manager.unblock_peer(peer_to_unblock) {
    println!("Successfully unblocked peer: {}", peer_to_unblock);
} else {
    println!("Peer was not blocked: {}", peer_to_unblock);
}
```

### Checking Blocked Peers
```rust
// Get current blocked peers list
let blocked_peers = peer_manager.blocked_peers();
println!("Currently blocked peers: {} peers", blocked_peers.len());

for peer_id in blocked_peers {
    println!("Blocked peer: {}", peer_id);
}

// Check if specific peer is blocked
let peer_id: PeerId = /* some peer */;
if blocked_peers.contains(&peer_id) {
    println!("Peer {} is currently blocked", peer_id);
}
```

## Heartbeat and Status Monitoring

### Periodic Status Logging
```rust
// The heartbeat automatically logs status every 30 seconds
// Output example:
// INFO subnets=4 peers=47 blocked_peers=2 "Network status"

// You can also trigger manual heartbeat
let connect_actions = peer_manager.heartbeat();
if let Some(actions) = connect_actions {
    println!("Heartbeat generated {} dial actions and {} discovery actions", 
             actions.dial.len(), actions.discover.len());
}
```

### Custom Status Reporting
```rust
use lighthouse_network::metrics;

// Access connection metrics
let connected_count = peer_manager.connection_manager.connected.len();
let target_peers = peer_manager.connection_manager.target_peers;
let blocked_count = peer_manager.blocked_peers().len();

println!("Network Status:");
println!("  Connected: {}/{} peers", connected_count, target_peers);
println!("  Blocked: {} peers", blocked_count);
println!("  Needed subnets: {} subnets", peer_manager.needed_subnets.len());

// Update custom metrics
metrics::set_gauge(&metrics::PEERS_CONNECTED, connected_count as i64);
```

## Advanced Usage Patterns

### Priority Peer Handling
```rust
// The peer manager automatically handles priority peers
// based on subnet requirements. No explicit API needed.
// Priority is determined by:
// 1. Peer serves needed subnets
// 2. Those subnets have < MIN_PEERS_PER_SUBNET (6) peers
// 3. Connection count is below max_with_priority_peers limit

// Example of how priority peer logic works internally:
let peer_id: PeerId = /* some peer */;
let peer_store = &peer_manager.peer_store.store();
let needed_subnets = &peer_manager.needed_subnets;

let is_priority = peer_manager.connection_manager
    .qualifies_for_priority(&peer_id, peer_store, needed_subnets);

if is_priority {
    println!("Peer {} qualifies for priority connection", peer_id);
}
```

### Connection Limit Management
```rust
// Check current connection status
let cm = &peer_manager.connection_manager;
let connected = cm.connected.len();
let target = cm.target_peers;
let max_priority = cm.max_with_priority_peers;

println!("Connection Status:");
println!("  Current: {} peers", connected);
println!("  Target: {} peers", target);
println!("  Max with priority: {} peers", max_priority);

// The connection manager automatically:
// - Allows 10% excess peers (PEER_EXCESS_FACTOR)
// - Reserves 20% more slots for priority peers (PRIORITY_PEER_EXCESS)
// - Maintains minimum outbound ratio (MIN_OUTBOUND_ONLY_FACTOR)
```

### Integration with Peer Scoring
```rust
// Typical integration with peer scoring system
struct NetworkManager {
    peer_manager: PeerManager,
    peer_scoring: PeerScoring,
}

impl NetworkManager {
    fn handle_peer_score_update(&mut self, peer_id: PeerId, score: f64) {
        const BLOCK_THRESHOLD: f64 = -100.0;
        
        if score < BLOCK_THRESHOLD {
            // Block the peer through peer manager
            if self.peer_manager.block_peer(peer_id) {
                println!("Blocked peer {} due to low score: {}", peer_id, score);
            }
        }
    }
    
    fn periodic_maintenance(&mut self) {
        // Trigger heartbeat which includes automatic peer unblocking
        if let Some(actions) = self.peer_manager.heartbeat() {
            self.execute_connect_actions(actions);
        }
        
        // Update peer scores based on recent behavior
        self.peer_scoring.update_scores();
    }
}
```

### Error Handling Examples
```rust
// Handle connection establishment errors
match swarm.dial(dial_opts) {
    Ok(_) => println!("Dialing peer..."),
    Err(libp2p::swarm::DialError::NoAddresses) => {
        println!("No addresses available for peer");
    }
    Err(libp2p::swarm::DialError::Denied { cause }) => {
        println!("Connection denied: {:?}", cause);
    }
    Err(e) => println!("Dial error: {:?}", e),
}

// Handle discovery timeouts
async fn discovery_with_timeout(subnet: SubnetId) -> Result<Vec<Enr>, DiscoveryError> {
    tokio::time::timeout(
        Duration::from_secs(30),
        discovery_service.find_subnet_peers(subnet)
    ).await
    .map_err(|_| DiscoveryError::Timeout)?
}
```

## Testing Examples

### Unit Test Setup
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    
    fn create_test_peer_manager() -> PeerManager {
        let config = Config {
            target_peers: 10,
            ..Default::default()
        };
        let one_epoch_duration = Duration::from_secs(384);
        PeerManager::new(&config, one_epoch_duration)
    }
    
    #[tokio::test]
    async fn test_subnet_joining() {
        let mut peer_manager = create_test_peer_manager();
        let subnet_id = SubnetId::new(42);
        
        let actions = peer_manager.join_subnet(subnet_id);
        
        // Initially no peers, should trigger discovery
        assert!(!actions.discover.is_empty());
        assert!(actions.discover.contains(&subnet_id));
    }
}
```

### Integration Test Example
```rust
#[tokio::test]
async fn test_peer_lifecycle() {
    let mut peer_manager = create_test_peer_manager();
    let test_enr = create_test_enr();
    let peer_id = test_enr.peer_id();
    
    // 1. Discovery
    let dial_opts = peer_manager.report_discovered_peer(test_enr);
    assert!(dial_opts.is_some());
    
    // 2. Connection (simulated)
    // In real usage, this would happen through swarm events
    peer_manager.connection_manager.on_connection_established(peer_id);
    
    // 3. Blocking for misbehavior
    assert!(peer_manager.block_peer(peer_id));
    assert!(peer_manager.blocked_peers().contains(&peer_id));
    
    // 4. Automatic unblocking after timeout
    // This would happen during heartbeat processing
    // after RETAIN_SCORE_EPOCH_MULTIPLIER epochs
}
```