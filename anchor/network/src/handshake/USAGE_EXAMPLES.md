# Handshake Component - Usage Examples

## Basic Usage

### 1. Creating a Handshake Behavior

```rust
use discv5::libp2p_identity::Keypair;
use anchor::network::handshake;

// Generate or load a keypair for the node
let keypair = Keypair::generate_ed25519();

// Create the handshake behavior
let handshake_behaviour = handshake::create_behaviour(keypair);
```

### 2. Setting Up Node Information

```rust
use anchor::network::handshake::node_info::{NodeInfo, NodeMetadata};

// Create node metadata with version and client information
let metadata = NodeMetadata {
    node_version: "anchor/v1.0.0".to_string(),
    execution_node: "geth/v1.13.0".to_string(),
    consensus_node: "lighthouse/v4.6.0".to_string(),
    subnets: "00000000000000000000000000000000".to_string(),
};

// Create node info for Holesky testnet
let node_info = NodeInfo::new(
    "00000502".to_string(), // Holesky network ID
    Some(metadata)
);
```

### 3. Integrating with Network Behavior

```rust
use libp2p::swarm::NetworkBehaviour;
use anchor::network::handshake;

#[derive(NetworkBehaviour)]
pub struct MyNetworkBehaviour {
    pub handshake: handshake::Behaviour,
    // ... other behaviors
}

impl MyNetworkBehaviour {
    pub fn new(keypair: Keypair) -> Self {
        Self {
            handshake: handshake::create_behaviour(keypair),
        }
    }
}
```

## Event Handling

### 4. Processing Handshake Events

```rust
use libp2p::swarm::{Swarm, SwarmEvent};
use anchor::network::handshake::{self, Event};

async fn handle_network_events(
    swarm: &mut Swarm<MyNetworkBehaviour>,
    node_info: &NodeInfo,
) {
    loop {
        match swarm.select_next_some().await {
            SwarmEvent::Behaviour(MyNetworkBehaviourEvent::Handshake(event)) => {
                if let Some(result) = handshake::handle_event(
                    node_info,
                    &mut swarm.behaviour_mut().handshake,
                    event,
                ) {
                    match result {
                        Ok(completed) => {
                            println!("Handshake completed with peer: {}", completed.peer_id);
                            println!("Their node version: {:?}", 
                                completed.their_info.metadata.map(|m| m.node_version));
                        }
                        Err(failed) => {
                            eprintln!("Handshake failed with peer {}: {:?}", 
                                failed.peer_id, failed.error);
                        }
                    }
                }
            }
            SwarmEvent::ConnectionEstablished { peer_id, endpoint, .. } => {
                // For outbound connections, initiate handshake
                if endpoint.is_dialer() {
                    handshake::initiate(
                        node_info,
                        &mut swarm.behaviour_mut().handshake,
                        peer_id,
                    );
                }
            }
            _ => {}
        }
    }
}
```

### 5. Handling Different Handshake Outcomes

```rust
use anchor::network::handshake::{Error, Completed, Failed};

fn handle_handshake_result(result: Result<Completed, Failed>) {
    match result {
        Ok(Completed { peer_id, their_info }) => {
            println!("✅ Handshake successful with {}", peer_id);
            
            // Access peer's network information
            println!("  Network: {}", their_info.network_id);
            
            if let Some(metadata) = their_info.metadata {
                println!("  Node version: {}", metadata.node_version);
                println!("  Execution client: {}", metadata.execution_node);
                println!("  Consensus client: {}", metadata.consensus_node);
                println!("  Subnets: {}", metadata.subnets);
            }
        }
        Err(Failed { peer_id, error }) => {
            match *error {
                Error::NetworkMismatch { ours, theirs } => {
                    println!("❌ Network mismatch with {}", peer_id);
                    println!("  Our network: {}", ours);
                    println!("  Their network: {}", theirs);
                }
                Error::NodeInfo(err) => {
                    println!("❌ NodeInfo error with {}: {}", peer_id, err);
                }
                Error::Inbound(err) => {
                    println!("❌ Inbound failure with {}: {:?}", peer_id, err);
                }
                Error::Outbound(err) => {
                    println!("❌ Outbound failure with {}: {:?}", peer_id, err);
                }
            }
        }
    }
}
```

## Advanced Usage

### 6. Dynamic Subnet Management

```rust
use subnet_service::SubnetId;
use anchor::network::handshake::node_info::NodeMetadata;

fn update_subnet_subscription(
    node_info: &mut NodeInfo,
    subnet: SubnetId,
    subscribed: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(metadata) = &mut node_info.metadata {
        metadata.set_subscribed(subnet, subscribed)?;
        println!("Updated subnet {} subscription: {}", *subnet, subscribed);
    }
    Ok(())
}

// Example usage:
let mut node_info = NodeInfo::new("00000502".to_string(), Some(default_metadata()));
update_subnet_subscription(&mut node_info, SubnetId::new(5), true)?;
```

### 7. Custom Error Handling with Logging

```rust
use tracing::{info, warn, error};

fn log_handshake_result(peer_id: libp2p::PeerId, result: Result<Completed, Failed>) {
    match result {
        Ok(completed) => {
            info!(
                peer_id = ?peer_id,
                network_id = %completed.their_info.network_id,
                "Handshake completed successfully"
            );
            
            if let Some(metadata) = &completed.their_info.metadata {
                info!(
                    peer_id = ?peer_id,
                    node_version = %metadata.node_version,
                    execution_node = %metadata.execution_node,
                    consensus_node = %metadata.consensus_node,
                    "Peer metadata received"
                );
            }
        }
        Err(failed) => {
            match *failed.error {
                Error::NetworkMismatch { ref ours, ref theirs } => {
                    warn!(
                        peer_id = ?peer_id,
                        our_network = %ours,
                        their_network = %theirs,
                        "Handshake failed: network mismatch"
                    );
                }
                _ => {
                    error!(
                        peer_id = ?peer_id,
                        error = ?failed.error,
                        "Handshake failed"
                    );
                }
            }
        }
    }
}
```

### 8. Creating Test NodeInfo Instances

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_node_info(network: &str, version: &str) -> NodeInfo {
        NodeInfo::new(
            network.to_string(),
            Some(NodeMetadata {
                node_version: version.to_string(),
                execution_node: "test-execution/v1.0.0".to_string(),
                consensus_node: "test-consensus/v1.0.0".to_string(),
                subnets: "00000000000000000000000000000000".to_string(),
            }),
        )
    }

    #[test]
    fn test_node_info_creation() {
        let node_info = create_test_node_info("00000502", "test/v1.0.0");
        assert_eq!(node_info.network_id, "00000502");
        assert!(node_info.metadata.is_some());
    }
}
```

### 9. Serialization and Deserialization

```rust
use anchor::network::handshake::node_info::{NodeInfo, NodeMetadata};

fn serialize_deserialize_example() -> Result<(), Box<dyn std::error::Error>> {
    // Create a NodeInfo instance
    let original = NodeInfo::new(
        "00000502".to_string(),
        Some(NodeMetadata {
            node_version: "anchor/v1.0.0".to_string(),
            execution_node: "geth/v1.13.0".to_string(),
            consensus_node: "lighthouse/v4.6.0".to_string(),
            subnets: "ffffffffffffffffffffffffffffffff".to_string(),
        }),
    );

    // Serialize to bytes
    let serialized = original.marshal()?;
    println!("Serialized size: {} bytes", serialized.len());

    // Deserialize back
    let deserialized = NodeInfo::unmarshal(&serialized)?;
    
    // Verify they match
    assert_eq!(original, deserialized);
    println!("Serialization roundtrip successful!");
    
    Ok(())
}
```

### 10. Creating Signed Envelopes

```rust
use discv5::libp2p_identity::Keypair;
use anchor::network::handshake::{envelope::Envelope, node_info::NodeInfo};

fn create_signed_envelope_example() -> Result<(), Box<dyn std::error::Error>> {
    // Create a keypair and node info
    let keypair = Keypair::generate_ed25519();
    let node_info = NodeInfo::new(
        "00000502".to_string(),
        Some(NodeMetadata::default()),
    );

    // Create a signed envelope
    let envelope = node_info.seal(&keypair)?;
    
    // Encode to bytes for transmission
    let encoded = envelope.encode_to_vec()?;
    println!("Envelope size: {} bytes", encoded.len());

    // Parse and verify the envelope
    let verified_envelope = Envelope::parse_and_verify(&encoded)?;
    let recovered_info = NodeInfo::unmarshal(&verified_envelope.payload)?;
    
    assert_eq!(node_info, recovered_info);
    println!("Envelope creation and verification successful!");
    
    Ok(())
}
```

## Integration Patterns

### 11. Network Configuration Integration

```rust
use anchor::config::Config;
use anchor::network::handshake::node_info::{NodeInfo, NodeMetadata};

fn create_node_info_from_config(config: &Config) -> NodeInfo {
    let metadata = NodeMetadata {
        node_version: format!("anchor/{}", env!("CARGO_PKG_VERSION")),
        execution_node: config.execution_client_info.clone(),
        consensus_node: config.consensus_client_info.clone(),
        subnets: "00000000000000000000000000000000".to_string(),
    };

    NodeInfo::new(
        config.network.domain_type.clone(),
        Some(metadata),
    )
}
```

### 12. Peer Compatibility Checking

```rust
fn check_peer_compatibility(our_info: &NodeInfo, their_info: &NodeInfo) -> bool {
    // Basic network compatibility
    if our_info.network_id != their_info.network_id {
        return false;
    }

    // Optional: Check version compatibility
    if let (Some(our_meta), Some(their_meta)) = (&our_info.metadata, &their_info.metadata) {
        // Example: Check if peer has a compatible version
        let compatible_versions = ["anchor/v1.0.0", "anchor/v1.1.0"];
        if !compatible_versions.contains(&their_meta.node_version.as_str()) {
            println!("Warning: Peer has potentially incompatible version: {}", 
                their_meta.node_version);
        }
    }

    true
}
```

### 13. Complete Network Setup Example

```rust
use libp2p::{Swarm, SwarmBuilder};
use anchor::network::handshake::{self, node_info::*};

async fn setup_network_with_handshake() -> Result<(), Box<dyn std::error::Error>> {
    // Generate keypair
    let keypair = Keypair::generate_ed25519();
    
    // Create node info
    let node_info = NodeInfo::new(
        "00000502".to_string(), // Holesky testnet
        Some(NodeMetadata {
            node_version: "anchor/v1.0.0".to_string(),
            execution_node: "geth/v1.13.0".to_string(),
            consensus_node: "lighthouse/v4.6.0".to_string(),
            subnets: "00000000000000000000000000000000".to_string(),
        }),
    );

    // Create network behavior
    let behaviour = MyNetworkBehaviour {
        handshake: handshake::create_behaviour(keypair.clone()),
    };

    // Create swarm
    let mut swarm = SwarmBuilder::with_existing_identity(keypair)
        .with_tokio()
        .with_tcp(
            libp2p::tcp::Config::default(),
            libp2p::noise::Config::new,
            libp2p::yamux::Config::default,
        )?
        .with_behaviour(|_| behaviour)?
        .build();

    // Start listening
    swarm.listen_on("/ip4/0.0.0.0/tcp/0".parse()?)?;

    // Main event loop
    loop {
        match swarm.select_next_some().await {
            SwarmEvent::Behaviour(MyNetworkBehaviourEvent::Handshake(event)) => {
                if let Some(result) = handshake::handle_event(
                    &node_info,
                    &mut swarm.behaviour_mut().handshake,
                    event,
                ) {
                    handle_handshake_result(result);
                }
            }
            SwarmEvent::ConnectionEstablished { peer_id, endpoint, .. } => {
                if endpoint.is_dialer() {
                    handshake::initiate(
                        &node_info,
                        &mut swarm.behaviour_mut().handshake,
                        peer_id,
                    );
                }
            }
            SwarmEvent::NewListenAddr { address, .. } => {
                println!("Listening on: {}", address);
            }
            _ => {}
        }
    }
}
```

These examples demonstrate the complete usage patterns for the handshake component, from basic setup to advanced integration with the broader network stack. The component provides a robust foundation for peer validation and network isolation in the Anchor SSV network.