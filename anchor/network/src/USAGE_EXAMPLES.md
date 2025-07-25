# Network Component Usage Examples

## Basic Network Setup

### Creating and Starting a Network Instance

```rust
use network::{Config, Network, DEFAULT_TCP_PORT, DEFAULT_DISC_PORT, DEFAULT_QUIC_PORT};
use std::path::PathBuf;
use std::sync::Arc;
use task_executor::TaskExecutor;
use types::{ChainSpec, MainnetEthSpec};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create network configuration
    let config = Config {
        network_dir: PathBuf::from(".anchor/network"),
        listen_addresses: lighthouse_network::ListenAddress::default(),
        enr_address: (None, None),
        enr_udp4_port: Some(DEFAULT_DISC_PORT.try_into().unwrap()),
        enr_tcp4_port: Some(DEFAULT_TCP_PORT.try_into().unwrap()),
        enr_quic4_port: Some(DEFAULT_QUIC_PORT.try_into().unwrap()),
        // ... other configuration fields
    };

    // Create chain specification
    let chain_spec = Arc::new(ChainSpec::mainnet());
    
    // Create task executor
    let executor = TaskExecutor::new();

    // Initialize network
    let mut network = Network::new(config, chain_spec, executor).await?;
    
    // Start networking
    network.start().await?;
    
    println!("Network started successfully!");
    Ok(())
}
```

## Gossip Messaging

### Publishing Messages

```rust
use gossipsub::IdentTopic;
use std::str::FromStr;

async fn publish_message(network: &mut Network) -> Result<(), Box<dyn std::error::Error>> {
    // Create a topic for SSV messages
    let topic = IdentTopic::new("ssv_messages");
    
    // Subscribe to the topic first
    network.subscribe(topic.clone()).await?;
    
    // Prepare message data
    let message_data = b"Hello SSV Network!".to_vec();
    
    // Publish the message
    network.publish(topic, message_data).await?;
    
    println!("Message published successfully!");
    Ok(())
}
```

### Receiving Messages

```rust
use futures::StreamExt;
use message_receiver::{MessageReceiver, Outcome};

async fn handle_incoming_messages(network: &mut Network) {
    // Get the network event stream
    let mut event_stream = network.event_stream();
    
    while let Some(event) = event_stream.next().await {
        match event {
            NetworkEvent::PubsubMessage { source, topic, message } => {
                println!("Received message from {}: {:?}", source, message);
                
                // Process the message through MessageReceiver
                let receiver = MessageReceiver::new();
                match receiver.process_message(&message).await {
                    Outcome::Accept => println!("Message accepted"),
                    Outcome::Reject => println!("Message rejected"),
                    Outcome::Ignore => println!("Message ignored"),
                }
            },
            _ => {} // Handle other event types
        }
    }
}
```

## Peer Management

### Connecting to Specific Peers

```rust
use libp2p::{PeerId, Multiaddr};
use std::str::FromStr;

async fn connect_to_peer(network: &mut Network) -> Result<(), Box<dyn std::error::Error>> {
    // Parse peer ID and address
    let peer_id = PeerId::from_str("12D3KooWExample...")?;
    let address = Multiaddr::from_str("/ip4/192.168.1.100/tcp/13001")?;
    
    // Attempt to connect
    network.dial_peer(peer_id, address).await?;
    
    println!("Connection attempt initiated");
    Ok(())
}
```

### Monitoring Connected Peers

```rust
async fn monitor_peers(network: &Network) {
    let connected_peers = network.connected_peers();
    println!("Currently connected to {} peers:", connected_peers.len());
    
    for peer_id in connected_peers {
        println!("  - {}", peer_id);
    }
}
```

### Disconnecting Peers

```rust
async fn disconnect_peer(network: &mut Network, peer_id: PeerId) {
    network.disconnect_peer(peer_id);
    println!("Disconnected from peer: {}", peer_id);
}
```

## Advanced Configuration

### Custom Network Configuration

```rust
use network::{Config, ListenAddr};
use std::net::{Ipv4Addr, Ipv6Addr};
use std::num::NonZeroU16;

fn create_custom_config() -> Config {
    Config {
        network_dir: PathBuf::from("/custom/network/path"),
        
        // Listen on specific interfaces
        listen_addresses: lighthouse_network::ListenAddress::V4(
            lighthouse_network::ListenAddr {
                addr: Ipv4Addr::new(0, 0, 0, 0),
                disc_port: 12001,
                quic_port: Some(13002),
                tcp_port: Some(13001),
            }
        ),
        
        // Configure ENR advertisement
        enr_address: (
            Some(Ipv4Addr::new(203, 0, 113, 1)), // Public IP
            Some(Ipv6Addr::from_str("2001:db8::1").unwrap())
        ),
        
        enr_udp4_port: NonZeroU16::new(12001),
        enr_tcp4_port: NonZeroU16::new(13001),
        enr_quic4_port: NonZeroU16::new(13002),
        
        // IPv6 configuration
        enr_udp6_port: NonZeroU16::new(12001),
        enr_tcp6_port: NonZeroU16::new(13001),
        enr_quic6_port: NonZeroU16::new(13002),
        
        // Additional configuration options...
    }
}
```

## Discovery and ENR Management

### Manual Peer Discovery

```rust
use subnet_service::SubnetId;

async fn discover_subnet_peers(
    network: &mut Network, 
    subnet_id: SubnetId
) -> Result<(), Box<dyn std::error::Error>> {
    // Trigger discovery for specific subnet
    network.discover_subnet_peers(subnet_id).await?;
    
    println!("Discovery query initiated for subnet: {}", subnet_id);
    Ok(())
}
```

### ENR Information Access

```rust
use network::Enr;

async fn display_enr_info(network: &Network) {
    if let Some(enr) = network.local_enr() {
        println!("Local ENR: {}", enr);
        println!("Node ID: {}", enr.node_id());
        println!("Sequence number: {}", enr.seq());
        
        // Display network addresses
        for (key, value) in enr.iter() {
            println!("  {}: {:?}", key, value);
        }
    }
}
```

## Error Handling

### Comprehensive Error Handling

```rust
use network::{NetworkError, PublishError};

async fn robust_network_operations(
    network: &mut Network
) -> Result<(), Box<dyn std::error::Error>> {
    // Handle network initialization errors
    match network.start().await {
        Ok(_) => println!("Network started successfully"),
        Err(NetworkError::Listen { address, source }) => {
            eprintln!("Failed to listen on {}: {}", address, source);
            return Err(Box::new(source));
        },
        Err(NetworkError::SwarmConfig(msg)) => {
            eprintln!("Swarm configuration error: {}", msg);
            return Err(Box::new(NetworkError::SwarmConfig(msg)));
        },
        Err(e) => return Err(Box::new(e)),
    }
    
    // Handle publishing errors
    let topic = gossipsub::IdentTopic::new("test_topic");
    let data = b"test message".to_vec();
    
    match network.publish(topic, data).await {
        Ok(_) => println!("Message published"),
        Err(PublishError::Duplicate) => {
            println!("Duplicate message, ignoring");
        },
        Err(PublishError::InsufficientPeers) => {
            println!("Not enough peers connected for publishing");
        },
        Err(e) => {
            eprintln!("Publish error: {}", e);
            return Err(Box::new(e));
        }
    }
    
    Ok(())
}
```

## Integration with Other Components

### Using with Message Receiver

```rust
use message_receiver::{MessageReceiver, MessageType};
use ssv_types::domain_type::DomainType;

async fn integrated_message_handling(network: &mut Network) {
    let receiver = MessageReceiver::new();
    let mut event_stream = network.event_stream();
    
    while let Some(event) = event_stream.next().await {
        if let NetworkEvent::PubsubMessage { source, message, .. } = event {
            // Process through message receiver
            match receiver.process_message(&message).await {
                Outcome::Accept => {
                    // Determine message type and handle accordingly
                    if let Ok(msg_type) = MessageType::from_bytes(&message) {
                        match msg_type {
                            MessageType::SSVMessage => {
                                println!("Processing SSV message from {}", source);
                                // Handle SSV-specific logic
                            },
                            MessageType::ValidatorRegistration => {
                                println!("Processing validator registration from {}", source);
                                // Handle validator registration
                            },
                            _ => println!("Unknown message type"),
                        }
                    }
                },
                Outcome::Reject => {
                    println!("Rejected message from {}", source);
                },
                Outcome::Ignore => {
                    // Message ignored, no action needed
                }
            }
        }
    }
}
```

### Subnet-based Operations

```rust
use subnet_service::{SubnetService, SubnetEvent, SUBNET_COUNT};

async fn subnet_operations(network: &mut Network) {
    // Subscribe to all subnet topics
    for subnet_id in 0..SUBNET_COUNT {
        let topic = gossipsub::IdentTopic::new(format!("subnet_{}", subnet_id));
        network.subscribe(topic).await.unwrap();
    }
    
    // Handle subnet events
    let mut subnet_events = network.subnet_event_stream();
    while let Some(event) = subnet_events.next().await {
        match event {
            SubnetEvent::PeerJoined { subnet_id, peer_id } => {
                println!("Peer {} joined subnet {}", peer_id, subnet_id);
            },
            SubnetEvent::PeerLeft { subnet_id, peer_id } => {
                println!("Peer {} left subnet {}", peer_id, subnet_id);
            },
            SubnetEvent::MessageReceived { subnet_id, message } => {
                println!("Message received on subnet {}", subnet_id);
                // Process subnet-specific message
            }
        }
    }
}
```

## Testing and Development

### Mock Network for Testing

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tokio_test;
    
    #[tokio::test]
    async fn test_network_initialization() {
        let config = Config::default();
        let chain_spec = Arc::new(ChainSpec::minimal());
        let executor = TaskExecutor::new();
        
        let result = Network::new(config, chain_spec, executor).await;
        assert!(result.is_ok());
    }
    
    #[tokio::test]
    async fn test_message_publishing() {
        let mut network = create_test_network().await;
        let topic = gossipsub::IdentTopic::new("test");
        let data = b"test data".to_vec();
        
        network.subscribe(topic.clone()).await.unwrap();
        let result = network.publish(topic, data).await;
        
        assert!(result.is_ok());
    }
    
    async fn create_test_network() -> Network {
        let config = Config {
            network_dir: std::env::temp_dir().join("test_network"),
            // ... test configuration
        };
        
        Network::new(
            config,
            Arc::new(ChainSpec::minimal()),
            TaskExecutor::new()
        ).await.unwrap()
    }
}
```