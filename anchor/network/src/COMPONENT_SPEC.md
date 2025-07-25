# Network Component Specification

## Component Overview

**Name**: `network`  
**Purpose**: Core P2P networking layer for Anchor SSV network  
**Language**: Rust  
**Framework**: libp2p with tokio async runtime

## Public API

### Main Struct: `Network`

```rust
pub struct Network {
    // Internal fields managed by the network implementation
}
```

### Key Methods

#### Initialization
- `new(config: Config, chain_spec: Arc<ChainSpec>, executor: TaskExecutor) -> Result<Self, NetworkError>`
- `start(&mut self) -> Result<(), NetworkError>`

#### Messaging
- `publish(&mut self, topic: IdentTopic, data: Vec<u8>) -> Result<(), PublishError>`
- `subscribe(&mut self, topic: IdentTopic) -> Result<(), SubscriptionError>`

#### Peer Management
- `connected_peers(&self) -> Vec<PeerId>`
- `dial_peer(&mut self, peer_id: PeerId, address: Multiaddr) -> Result<(), NetworkError>`
- `disconnect_peer(&mut self, peer_id: PeerId)`

### Configuration Struct: `Config`

```rust
pub struct Config {
    pub network_dir: PathBuf,
    pub listen_addresses: ListenAddress,
    pub enr_address: (Option<Ipv4Addr>, Option<Ipv6Addr>),
    pub enr_udp4_port: Option<NonZeroU16>,
    pub enr_quic4_port: Option<NonZeroU16>,
    pub enr_tcp4_port: Option<NonZeroU16>,
    // Additional IPv6 and configuration fields...
}
```

### Error Types

```rust
pub enum NetworkError {
    Listen { address: Multiaddr, source: TransportError },
    SwarmConfig(String),
    DnsTransport(std::io::Error),
    // Additional error variants...
}
```

## Internal Architecture

### Core Components

#### 1. AnchorBehaviour
- **Purpose**: Combines multiple libp2p behaviors
- **Behaviors**:
  - `identify`: Peer identification
  - `ping`: Connection health checks  
  - `gossipsub`: Message propagation
  - `discovery`: Peer discovery (discv5)
  - `peer_manager`: Advanced peer lifecycle management
  - `handshake`: Secure peer authentication

#### 2. Discovery System
- **Protocol**: discv5 for distributed peer discovery
- **Features**:
  - Subnet-aware peer queries
  - ENR-based peer advertisement
  - IPv4/IPv6 dual-stack support
  - Configurable query parameters

#### 3. Transport Layer
- **Protocols**: TCP, QUIC over UDP
- **Encryption**: Noise protocol
- **Multiplexing**: yamux (TCP only)
- **Features**: DNS resolution, connection timeouts

#### 4. Peer Manager
- **Responsibilities**:
  - Connection lifecycle management
  - Peer blocking and reputation
  - Heartbeat monitoring
  - Connection limits enforcement

#### 5. Scoring System
- **Purpose**: Peer quality assessment and spam protection
- **Components**:
  - Peer score configuration
  - Topic score configuration  
  - Threshold-based peer management

## Data Flow

### 1. Network Initialization
```
Config → Transport → Swarm → Behaviour → Network
```

### 2. Peer Discovery
```
Discovery Query → discv5 → ENR Response → Peer Connection → Handshake
```

### 3. Message Flow
```
Application → Network.publish() → Gossipsub → Peers → Network Event → Application
```

### 4. Peer Management
```
Connection Event → Peer Manager → Scoring → Action (Keep/Block/Disconnect)
```

## Constants and Defaults

```rust
pub const DEFAULT_TCP_PORT: u16 = 13001;
pub const DEFAULT_DISC_PORT: u16 = 12001;
pub const DEFAULT_QUIC_PORT: u16 = 13002;
const MAX_TRANSMIT_SIZE_BYTES: usize = 5_000_000;
const TARGET_PEERS_FOR_GROUPED_QUERY: usize = 6;
```

## Dependencies

### External Crates
- `libp2p`: Core P2P networking (v0.x)
- `discv5`: Ethereum discovery protocol
- `gossipsub`: Publish-subscribe messaging  
- `tokio`: Async runtime
- `lighthouse_network`: Ethereum networking utilities

### Internal Crates
- `message_receiver`: Message processing
- `subnet_service`: Subnet management
- `task_executor`: Task coordination
- `ssv_types`: SSV-specific types
- `types`: Common type definitions

## Thread Safety

- All public methods are designed for single-threaded use within tokio context
- Internal synchronization handled by libp2p swarm
- Message passing used for cross-task communication
- No explicit locking required by users

## Performance Characteristics

### Scalability
- Supports thousands of concurrent connections
- Efficient message routing with topic-based filtering
- Connection pooling and reuse

### Resource Usage
- Memory: Scales with peer count and message buffer size
- Network: Configurable bandwidth limits
- CPU: Async processing minimizes blocking operations

### Latency
- Message propagation: ~100-500ms (network dependent)
- Peer discovery: ~1-5 seconds for new peers
- Connection establishment: ~100-1000ms

## Security Model

### Authentication
- Noise protocol for transport encryption
- Peer identity verification through handshake
- ENR signature validation

### Authorization
- Peer scoring for reputation management
- Topic-based access control
- Connection limits per peer

### Attack Mitigation
- Rate limiting on messages and connections
- Peer banning for malicious behavior
- Message validation and size limits

## Monitoring and Observability

### Metrics
- Connection counts and states
- Message throughput and latency
- Peer discovery success rates
- Error rates by category

### Logging
- Structured logging with tracing crate
- Configurable log levels
- Peer and connection event tracking

### Health Checks
- Ping/pong for connection health
- Heartbeat monitoring
- Discovery service status