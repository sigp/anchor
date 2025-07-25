# Peer Manager Component - AI Documentation

## Overview
The Peer Manager is a core networking component responsible for managing peer connections, discovery, blocking, and subnet-based peer selection in the Anchor network. It serves as a modular, coordinated system that ensures optimal peer connectivity for validator duties and network health.

## Purpose
- **Connection Management**: Maintains optimal peer count with intelligent connection limits
- **Subnet-aware Peer Selection**: Ensures adequate peers for required validator subnets
- **Peer Blocking/Unblocking**: Protects against misbehaving peers with automatic recovery
- **Discovery Coordination**: Orchestrates peer discovery based on subnet needs
- **Priority Peer Handling**: Gives preference to peers needed for validator duties

## Architecture

### Core Structure
```rust
pub struct PeerManager {
    peer_store: peer_store::Behaviour<MemoryStore<Enr>>,
    connection_manager: ConnectionManager,
    heartbeat_manager: HeartbeatManager,
    blocking_manager: BlockingManager,
    needed_subnets: HashSet<SubnetId>,
}
```

### Modular Components

#### 1. ConnectionManager (`connection.rs`)
- **Purpose**: Manages connection limits and peer prioritization
- **Key Features**:
  - Dynamic connection limits based on target peers and excess factors
  - Priority peer logic for subnet-specific requirements
  - Subnet-aware peer counting and qualification

#### 2. BlockingManager (`blocking.rs`)
- **Purpose**: Handles peer blocking with automatic time-based recovery
- **Key Features**:
  - Timestamp-based blocking with automatic unblocking
  - Integration with libp2p's allow_block_list behavior
  - Configurable retain_score duration (RETAIN_SCORE_EPOCH_MULTIPLIER epochs)

#### 3. PeerDiscovery (`discovery.rs`)
- **Purpose**: Manages peer discovery and subnet-based peer selection
- **Key Features**:
  - Subnet-aware peer discovery with overdial factors
  - Random peer selection for fairness
  - Action-based response system (dial/discover)

#### 4. HeartbeatManager (`heartbeat.rs`)
- **Purpose**: Provides periodic status reporting and peer maintenance
- **Key Features**:
  - 30-second heartbeat intervals
  - Triggers peer score checks and connection maintenance

## Key Algorithms

### Peer Selection Algorithm
1. **Subnet Analysis**: Count connected peers per required subnet
2. **Need Calculation**: Apply MIN_PEERS_PER_SUBNET and PEER_OVERDIAL_FACTOR
3. **Candidate Filtering**: Filter by availability, addresses, and blocking status
4. **Random Selection**: Shuffle candidates for fairness
5. **Action Generation**: Create dial/discover actions based on needs

### Priority Peer Logic
```rust
// Peers qualify for priority if they serve needed subnets with insufficient coverage
pub fn qualifies_for_priority(&self, peer_id: &PeerId, peer_store: &MemoryStore<Enr>, needed_subnets: &HashSet<SubnetId>) -> bool
```

### Connection Limits
- **Base Target**: `config.target_peers`
- **Excess Factor**: 10% additional peers (`PEER_EXCESS_FACTOR = 0.1`)
- **Priority Excess**: 20% more for subnet duties (`PRIORITY_PEER_EXCESS = 0.2`)
- **Minimum per Subnet**: 6 peers (`MIN_PEERS_PER_SUBNET`)

## Network Behavior Integration

### libp2p NetworkBehaviour Implementation
The PeerManager implements `NetworkBehaviour` with:
- **Connection Handling**: All connection phases (pending/established, inbound/outbound)
- **Event Processing**: Swarm events and connection state changes
- **Polling**: Heartbeat timing and sub-component event forwarding

### Event Flow
1. **Connection Events** → ConnectionManager → Metrics Update
2. **Blocking Events** → BlockingManager → Connection Denial/Closure
3. **Heartbeat Events** → Status Logging + Peer Discovery Actions
4. **Discovery Events** → PeerStore Updates + Dial Decisions

## Configuration

### Key Parameters
- `target_peers`: Target number of connected peers
- `one_epoch_duration`: Used for blocking timeout calculations
- `HEARTBEAT_INTERVAL`: 30 seconds between status checks
- `MIN_PEERS_PER_SUBNET`: 6 peers minimum per subnet
- `PEER_OVERDIAL_FACTOR`: 2x overdial when seeking subnet peers

### Integration Points
- **Config**: Network configuration (target peers, etc.)
- **EnrExt**: ENR record extensions for subnet information
- **SubnetService**: Validator subnet tracking
- **PeerStore**: Persistent peer information storage

## Error Handling
- **Connection Denials**: Graceful handling with priority peer exceptions
- **Blocking Timeout**: Automatic peer unblocking after retain_score duration
- **Discovery Failures**: Fallback to additional discovery queries
- **Network Events**: Robust event delegation to sub-components

## Performance Characteristics
- **Memory**: Bounded by target peer limits and blocking history
- **CPU**: Periodic heartbeat processing (30s intervals)
- **Network**: Controlled connection attempts with overdial protection
- **Scalability**: Linear with target peer count and subnet requirements

## Testing
The component includes comprehensive unit tests covering:
- Peer blocking/unblocking scenarios
- Timeout-based automatic unblocking
- Multi-peer blocking coordination
- Connection limit enforcement
- Subnet-based peer selection

## Dependencies
- **libp2p**: Core networking and behavior framework
- **discv5**: ENR records and peer identity
- **peer_store**: Persistent peer information
- **subnet_service**: Validator subnet management
- **lighthouse_network**: Metrics and network utilities