# Handshake Component - Technical Specification

## Component Overview

**Module Path**: `anchor::network::handshake`  
**Protocol**: `/ssv/info/0.0.1`  
**Transport**: libp2p request-response  
**Purpose**: Peer compatibility validation and metadata exchange  

## API Specification

### Public Types

#### `Behaviour`
```rust
pub type Behaviour = RequestResponseBehaviour<Codec>;
```
- **Description**: libp2p network behavior for handling handshake protocol
- **Based on**: `libp2p::request_response::Behaviour`
- **Codec**: Custom `Codec` implementation for NodeInfo serialization

#### `Event`
```rust
pub type Event = <Behaviour as NetworkBehaviour>::ToSwarm;
```
- **Description**: Events emitted by the handshake behavior
- **Types**: Message requests/responses, inbound/outbound failures

#### `Error`
```rust
pub enum Error {
    NetworkMismatch { ours: String, theirs: String },
    NodeInfo(node_info::Error),
    Inbound(InboundFailure),
    Outbound(OutboundFailure),
}
```

#### `Completed`
```rust
pub struct Completed {
    pub peer_id: PeerId,
    pub their_info: NodeInfo,
}
```

#### `Failed`
```rust
pub struct Failed {
    pub peer_id: PeerId,
    pub error: Box<Error>,
}
```

### Core Functions

#### `create_behaviour(keypair: Keypair) -> Behaviour`
- **Purpose**: Creates libp2p behavior for handshake handling
- **Parameters**: Node's cryptographic keypair
- **Returns**: Configured request-response behavior
- **Protocol**: `/ssv/info/0.0.1`

#### `handle_event(our_node_info: &NodeInfo, behaviour: &mut Behaviour, event: Event) -> Option<Result<Completed, Failed>>`
- **Purpose**: Processes handshake events and validates peers
- **Parameters**: 
  - `our_node_info`: Local node information
  - `behaviour`: Mutable reference to handshake behavior
  - `event`: Incoming handshake event
- **Returns**: Optional result indicating success or failure

#### `initiate(our_node_info: &NodeInfo, behaviour: &mut Behaviour, peer_id: PeerId)`
- **Purpose**: Initiates handshake with a specific peer
- **Parameters**:
  - `our_node_info`: Local node information to send
  - `behaviour`: Mutable reference to handshake behavior  
  - `peer_id`: Target peer to handshake with
- **Side Effects**: Sends handshake request via libp2p

## Data Structures

### NodeInfo
```rust
pub struct NodeInfo {
    pub network_id: String,
    pub metadata: Option<NodeMetadata>,
}
```

**Fields**:
- `network_id`: Network identifier (e.g., "00000502" for Holesky)
- `metadata`: Optional node metadata containing version and client info

**Constants**:
- `DOMAIN`: `"ssv"` - Message signing domain
- `CODEC`: `b"ssv/nodeinfo"` - Message payload type identifier

**Methods**:
- `new(network_id: String, metadata: Option<NodeMetadata>) -> Self`
- `marshal(&self) -> Result<Vec<u8>, Error>` - Serialize to JSON bytes
- `unmarshal(data: &[u8]) -> Result<NodeInfo, Error>` - Deserialize from JSON bytes
- `seal(&self, keypair: &Keypair) -> Result<Envelope, Error>` - Create signed envelope

### NodeMetadata
```rust
pub struct NodeMetadata {
    pub node_version: String,     // Node software version
    pub execution_node: String,   // Execution client info
    pub consensus_node: String,   // Consensus client info  
    pub subnets: String,         // Hex-encoded subnet subscriptions
}
```

**Methods**:
- `set_subscribed(&mut self, subnet: SubnetId, subscribed: bool) -> Result<(), Error>`

### Envelope
```rust
pub struct Envelope {
    pub public_key: Vec<u8>,     // Protobuf-encoded public key
    pub payload_type: Vec<u8>,   // Payload type identifier
    pub payload: Vec<u8>,        // Serialized NodeInfo
    pub signature: Vec<u8>,      // Cryptographic signature
}
```

**Methods**:
- `encode_to_vec(&self) -> Result<Vec<u8>, Error>` - Serialize to protobuf bytes
- `parse_and_verify(bytes: &[u8]) -> Result<Envelope, Error>` - Deserialize and verify signature

### Codec
```rust
pub struct Codec {
    keypair: Keypair,
}
```

**Implementation**: `libp2p::request_response::Codec`
- **Protocol**: `StreamProtocol`
- **Request/Response**: Both `NodeInfo`
- **Max Size**: 1024 bytes

## Protocol Specification

### Message Flow

1. **Connection Establishment**
   - libp2p connection established between peers
   - Outbound connection triggers automatic handshake initiation

2. **Handshake Request**
   ```
   Initiator → Responder: NodeInfo (signed envelope)
   ```
   - Contains initiator's network ID and metadata
   - Cryptographically signed with node's keypair

3. **Handshake Response**
   ```
   Responder → Initiator: NodeInfo (signed envelope)
   ```
   - Contains responder's network ID and metadata
   - Also cryptographically signed

4. **Validation**
   - Both peers validate network compatibility
   - Signature verification ensures authenticity
   - Network ID must match for success

### Wire Format

#### Envelope Structure (Protobuf)
```protobuf
message Envelope {
    bytes public_key = 1;    // Node's public key (protobuf encoded)
    bytes payload_type = 2;  // Always "ssv/nodeinfo"
    bytes payload = 3;       // JSON-serialized NodeInfo
    bytes signature = 5;     // Signature of unsigned data
}
```

#### NodeInfo JSON Format
```json
{
  "Entries": [
    "",                    // Legacy field (empty)
    "0x00000502",         // Network ID with 0x prefix
    "{...metadata...}"    // JSON-encoded NodeMetadata (optional)
  ]
}
```

#### NodeMetadata JSON Format
```json
{
  "NodeVersion": "v1.0.0",
  "ExecutionNode": "geth/v1.10.8", 
  "ConsensusNode": "lighthouse/v1.5.0",
  "Subnets": "00000000000000000000000000000000"
}
```

### Signature Scheme

**Unsigned Data Construction**:
```
unsigned = domain || payload_type || payload
```
- `domain`: `"ssv"` (3 bytes)
- `payload_type`: `"ssv/nodeinfo"` (12 bytes)  
- `payload`: JSON-serialized NodeInfo

**Signature Algorithm**: Based on keypair type (Ed25519, Secp256k1, etc.)

## Error Handling

### Error Categories

1. **Network Mismatch** (`Error::NetworkMismatch`)
   - Peers have different `network_id` values
   - Results in handshake failure
   - Peer is not added to compatible peer set

2. **Serialization Errors** (`Error::NodeInfo`)
   - JSON parsing failures
   - Invalid data format
   - Missing required fields

3. **Network Failures** (`Error::Inbound`/`Error::Outbound`)
   - Connection drops during handshake
   - Timeout conditions
   - Transport-level errors

4. **Cryptographic Errors**
   - Signature verification failure
   - Public key decoding errors
   - Invalid envelope format

### Recovery Behavior

- **Failed handshakes**: Peer marked as incompatible
- **Network errors**: Connection may be retried
- **Validation errors**: Peer is rejected permanently
- **Timeout handling**: Managed by libp2p request-response

## Performance Characteristics

### Message Size Limits
- **Maximum envelope size**: 1024 bytes
- **Typical NodeInfo size**: ~300-500 bytes
- **Protobuf overhead**: ~50-100 bytes

### Timing Characteristics
- **Handshake duration**: Typically 10-50ms
- **Timeout**: Configured by libp2p (default: 10s)
- **Retry behavior**: No automatic retries

### Resource Usage
- **Memory**: Minimal per-peer state
- **CPU**: Lightweight cryptographic operations
- **Network**: Single request-response exchange

## Security Properties

### Threat Model
- **Impersonation**: Prevented by signature verification
- **Network isolation**: Enforced by network ID validation
- **DoS protection**: Message size limits
- **Replay attacks**: Not specifically addressed (stateless protocol)

### Cryptographic Guarantees
- **Authentication**: Messages are signed by sender
- **Integrity**: Tampering detected via signature verification
- **Non-repudiation**: Signatures provide proof of origin

## Integration Requirements

### Dependencies
- `libp2p`: Core networking and request-response
- `discv5`: Key management and cryptography
- `serde`/`serde_json`: JSON serialization
- `quick-protobuf`: Protobuf handling

### Configuration
- **Network ID**: Must match target network (e.g., Holesky: "00000502")
- **Keypair**: Node's cryptographic identity
- **Metadata**: Current node version and client information

### Lifecycle
1. **Initialization**: Create behavior with node's keypair
2. **Event Processing**: Handle events in main network loop
3. **Peer Management**: Store handshake results for routing decisions
4. **Updates**: Refresh NodeInfo when metadata changes

## Testing Specification

### Test Coverage
- **Successful handshakes** between compatible peers
- **Network mismatch detection** and proper error handling
- **Serialization roundtrip** testing for data integrity
- **Signature verification** for security validation
- **Integration testing** with libp2p swarms

### Test Utilities
- Mock keypair generation for test scenarios
- Helper functions for creating test NodeInfo instances
- Swarm testing utilities for integration tests

This specification defines the complete technical interface and behavior of the handshake component within the Anchor SSV network stack.