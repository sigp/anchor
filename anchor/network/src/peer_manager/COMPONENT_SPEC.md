# Peer Manager Component Specification

## Component Identity
- **Name**: PeerManager
- **Module Path**: `anchor::network::peer_manager`
- **Type**: libp2p NetworkBehaviour Implementation
- **Version**: Part of Anchor Network Stack

## Public API

### Main Structure
```rust
pub struct PeerManager {
    peer_store: peer_store::Behaviour<MemoryStore<Enr>>,
    connection_manager: ConnectionManager,
    heartbeat_manager: HeartbeatManager,
    blocking_manager: BlockingManager,
    needed_subnets: HashSet<SubnetId>,
}
```

### Constructor
```rust
pub fn new(config: &Config, one_epoch_duration: Duration) -> Self
```
**Parameters:**
- `config: &Config` - Network configuration containing target peers and other settings
- `one_epoch_duration: Duration` - Epoch duration for blocking timeout calculations

### Core Methods

#### Peer Discovery
```rust
pub fn report_discovered_peer(&mut self, enr: Enr) -> Option<DialOpts>
```
- **Purpose**: Process discovered peer and determine if we should dial
- **Returns**: `Some(DialOpts)` if peer should be dialed, `None` otherwise
- **Side Effects**: Updates peer store with ENR and addresses

#### Subnet Management
```rust
pub fn join_subnet(&mut self, subnet_id: SubnetId) -> ConnectActions
```
- **Purpose**: Track subnet as needed and find peers for it
- **Returns**: Actions to dial peers or start discovery
- **Side Effects**: Adds subnet to `needed_subnets` set

#### Heartbeat Processing
```rust
pub fn heartbeat(&mut self) -> Option<ConnectActions>
```
- **Purpose**: Periodic maintenance and status reporting
- **Returns**: Actions if subnets need more peers
- **Side Effects**: Logs network status, unblocks expired peers

#### Peer Blocking
```rust
pub fn block_peer(&mut self, peer_id: PeerId) -> bool
pub fn unblock_peer(&mut self, peer_id: PeerId) -> bool
pub fn blocked_peers(&self) -> &HashSet<PeerId>
```
- **Purpose**: Manage peer blocking for misbehavior protection
- **Returns**: Success status for block/unblock operations
- **Side Effects**: Updates internal blocking state and timestamps

## Data Structures

### ConnectActions
```rust
pub struct ConnectActions {
    pub dial: Vec<DialOpts>,
    pub discover: Vec<SubnetId>,
}
```
- **Purpose**: Encapsulates actions the network should take
- **Methods**: `none()`, `is_empty()`

### Event Types
```rust
pub enum Event {
    PeerStore(peer_store::Event<memory_store::Event>),
    Heartbeat(crate::peer_manager::heartbeat::Event),
}
```

### Heartbeat Event
```rust
pub struct Event {
    pub connect_actions: Option<ConnectActions>,
    pub check_peer_scores: bool,
}
```

## Sub-Components Specifications

### ConnectionManager
**File**: `connection.rs`
**Purpose**: Connection limit management and peer prioritization

**Key Constants:**
```rust
const PEER_EXCESS_FACTOR: f32 = 0.1;           // 10% excess peers allowed
const MIN_OUTBOUND_ONLY_FACTOR: f32 = 0.2;     // 20% minimum outbound threshold
const PRIORITY_PEER_EXCESS: f32 = 0.2;         // 20% extra for priority peers
const MIN_PEERS_PER_SUBNET: usize = 6;         // Minimum peers per subnet
```

**Key Methods:**
```rust
pub fn should_dial_peer(&self, peer_id: &PeerId, peer_store: &MemoryStore<Enr>, needed_subnets: &HashSet<SubnetId>, blocked_peers: &HashSet<PeerId>) -> bool
pub fn qualifies_for_priority(&self, peer_id: &PeerId, peer_store: &MemoryStore<Enr>, needed_subnets: &HashSet<SubnetId>) -> bool
pub fn count_peers_for_subnets(&self, subnet_ids: &[SubnetId], peer_store: &MemoryStore<Enr>) -> Vec<usize>
```

### BlockingManager
**File**: `blocking.rs`
**Purpose**: Peer blocking with automatic timeout-based recovery

**Key Methods:**
```rust
pub fn block_peer(&mut self, peer_id: PeerId) -> bool
pub fn unblock_peer(&mut self, peer_id: PeerId) -> bool
pub fn check_and_unblock_expired_peers(&mut self)
pub fn blocked_peers(&self) -> &HashSet<PeerId>
```

**Timeout Calculation:**
```rust
retain_score_duration = one_epoch_duration * RETAIN_SCORE_EPOCH_MULTIPLIER
```

### PeerDiscovery
**File**: `discovery.rs`
**Purpose**: Subnet-aware peer discovery and selection

**Key Constants:**
```rust
const PEER_OVERDIAL_FACTOR: usize = 2;         // Overdial multiplier
const MIN_PEERS_PER_SUBNET: usize = 6;         // Minimum subnet peers
```

**Key Methods:**
```rust
pub fn process_discovered_peer(enr: Enr, peer_store: &mut MemoryStore<Enr>, connection_manager: &ConnectionManager, needed_subnets: &HashSet<SubnetId>, blocked_peers: &HashSet<PeerId>) -> Option<DialOpts>
pub fn track_subnet_peers(subnet_id: SubnetId, needed_subnets: &mut HashSet<SubnetId>, peer_store: &MemoryStore<Enr>, connection_manager: &ConnectionManager, blocked_peers: &HashSet<PeerId>) -> ConnectActions
pub fn check_subnet_peers(needed_subnets: &HashSet<SubnetId>, peer_store: &MemoryStore<Enr>, connection_manager: &ConnectionManager, blocked_peers: &HashSet<PeerId>) -> Option<ConnectActions>
```

### HeartbeatManager
**File**: `heartbeat.rs`
**Purpose**: Periodic status reporting and maintenance timing

**Constants:**
```rust
const HEARTBEAT_INTERVAL: u64 = 30;            // 30 seconds between heartbeats
```

**Key Methods:**
```rust
pub fn poll_tick(&mut self, cx: &mut Context<'_>) -> Poll<Instant>
```

## NetworkBehaviour Implementation

### Connection Handler
```rust
type ConnectionHandler = dummy::ConnectionHandler;
type ToSwarm = Event;
```

### Required NetworkBehaviour Methods

#### Connection Lifecycle
```rust
fn handle_pending_inbound_connection(&mut self, connection_id: ConnectionId, local_addr: &Multiaddr, remote_addr: &Multiaddr) -> Result<(), ConnectionDenied>
fn handle_established_inbound_connection(&mut self, connection_id: ConnectionId, peer: PeerId, local_addr: &Multiaddr, remote_addr: &Multiaddr) -> Result<THandler<Self>, ConnectionDenied>
fn handle_pending_outbound_connection(&mut self, connection_id: ConnectionId, maybe_peer: Option<PeerId>, addresses: &[Multiaddr], effective_role: Endpoint) -> Result<Vec<Multiaddr>, ConnectionDenied>
fn handle_established_outbound_connection(&mut self, connection_id: ConnectionId, peer: PeerId, addr: &Multiaddr, role_override: Endpoint, port_use: PortUse) -> Result<THandler<Self>, ConnectionDenied>
```

#### Event Processing
```rust
fn on_swarm_event(&mut self, event: FromSwarm)
fn on_connection_handler_event(&mut self, peer_id: PeerId, connection_id: ConnectionId, event: THandlerOutEvent<Self>)
fn poll(&mut self, cx: &mut Context<'_>) -> Poll<ToSwarm<Self::ToSwarm, THandlerInEvent<Self>>>
```

## State Management

### Internal State
- `peer_store`: Persistent peer information and ENR records
- `connection_manager.connected`: Currently connected peer set
- `needed_subnets`: Subnets requiring peer coverage
- `blocking_manager.blocked_peers_timestamps`: Blocked peer tracking with timestamps

### State Transitions
1. **Peer Discovery**: ENR → PeerStore → DialOpts (if needed)
2. **Connection Establishment**: Pending → Established → Connected Set
3. **Peer Blocking**: Misbehavior → Blocked → Timestamp Tracking → Auto-unblock
4. **Subnet Tracking**: Join Request → needed_subnets → Discovery Actions

## Dependencies and Integration

### Required Dependencies
```rust
use discv5::libp2p_identity::PeerId;
use libp2p::{Multiaddr, swarm::*, core::*};
use peer_store::memory_store::MemoryStore;
use subnet_service::SubnetId;
use lighthouse_network::EnrExt;
```

### Integration Points
- **Network Layer**: Receives events from libp2p swarm
- **Discovery**: Processes ENR records from discovery protocol
- **Scoring**: Provides blocked peer information to scoring system
- **Metrics**: Updates connection count metrics via lighthouse_network

## Performance Characteristics

### Time Complexity
- **Peer Discovery Processing**: O(1)
- **Subnet Peer Counting**: O(connected_peers × subnets)
- **Blocking Operations**: O(1)
- **Heartbeat Processing**: O(needed_subnets × peer_store_size)

### Space Complexity
- **Connected Peers**: O(target_peers)
- **Blocked Peers**: O(blocked_count)
- **Needed Subnets**: O(validator_duties)
- **Peer Store**: O(discovered_peers)

### Configuration Limits
- **Max Connected**: `target_peers × (1 + PEER_EXCESS_FACTOR + PRIORITY_PEER_EXCESS)`
- **Max Inbound**: `target_peers × (1 + PEER_EXCESS_FACTOR - MIN_OUTBOUND_ONLY_FACTOR)`
- **Max Outbound**: `target_peers × (1 + PEER_EXCESS_FACTOR)`

## Thread Safety and Concurrency
- **Single-threaded**: Designed for use within libp2p's single-threaded executor
- **Async**: Integrates with tokio's async runtime for timing operations
- **Event-driven**: Responds to network events and periodic heartbeats

## Error Handling Patterns
- **Connection Denials**: Return `ConnectionDenied` with appropriate reason
- **Missing Data**: Use `Option` types with graceful fallbacks
- **Timeouts**: Automatic cleanup via heartbeat processing
- **Resource Limits**: Enforce limits with priority peer exceptions