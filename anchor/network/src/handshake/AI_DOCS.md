# Handshake Component - AI Documentation

## Overview

The handshake component implements a peer-to-peer handshake protocol for the Anchor SSV network. It ensures that nodes only establish connections with compatible peers on the same network by exchanging and validating node information during connection establishment.

## Core Purpose

- **Network Isolation**: Prevents nodes from different SSV networks from communicating
- **Peer Validation**: Verifies compatibility before establishing full peer relationships
- **Metadata Exchange**: Shares node version, client information, and subnet subscriptions
- **Security**: Uses cryptographic signatures to verify node authenticity

## Architecture

The handshake component consists of several key modules:

### 1. Main Module (`mod.rs`)
- **Protocol**: `/ssv/info/0.0.1` - libp2p request-response protocol
- **Core Types**: `Completed`, `Failed`, `Error` for handshake outcomes
- **Event Handling**: Processes handshake requests, responses, and failures
- **Validation Logic**: Ensures network compatibility between peers

### 2. Codec (`codec.rs`)
- **Serialization**: Handles encoding/decoding of handshake messages
- **Security**: Integrates cryptographic signing and verification
- **Size Limits**: Enforces maximum message size (1024 bytes)
- **Async I/O**: Supports async read/write operations

### 3. Node Info (`node_info.rs`)
- **NodeInfo Structure**: Contains network ID and metadata
- **NodeMetadata**: Node version, execution/consensus clients, subnet info
- **Serialization Format**: JSON-based format compatible with legacy systems
- **Subnet Management**: Dynamic subnet subscription tracking

### 4. Envelope (`envelope/`)
- **Message Wrapper**: Protobuf-based secure message envelope
- **Signature Verification**: Cryptographic validation of messages
- **Protocol Buffer**: Defined schema for wire format

## Key Features

### Network Compatibility Validation
```rust
fn verify_node_info(ours: &NodeInfo, theirs: &NodeInfo) -> Result<(), Error> {
    if ours.network_id != theirs.network_id {
        return Err(Error::NetworkMismatch {
            ours: ours.network_id.clone(),
            theirs: theirs.network_id.clone(),
        });
    }
    Ok(())
}
```

### Automatic Handshake Initiation
- Triggers automatically on outbound connection establishment
- Uses libp2p request-response pattern
- Handles both initiator and responder roles

### Cryptographic Security
- All messages are cryptographically signed
- Public key verification ensures message authenticity
- Protection against tampering and impersonation

### Dynamic Metadata Updates
- Node information can be updated at runtime
- Subnet subscriptions are dynamically managed
- Reflects current node capabilities and state

## Integration with Network Stack

### Event Flow
1. **Connection Established** → Handshake initiated for outbound connections
2. **Request Sent** → Node sends its NodeInfo to peer
3. **Response Received** → Peer responds with their NodeInfo
4. **Validation** → Network compatibility is verified
5. **Result Handled** → Success/failure processed by network layer

### Network Behavior Integration
- Integrated as `handshake::Behaviour` in `AnchorBehaviour`
- Events processed in main network event loop
- Results stored for peer management decisions

### Configuration Dependencies
- **Network ID**: Derived from SSV domain type configuration
- **Node Metadata**: Reflects actual client versions and capabilities
- **Keypair**: Uses node's cryptographic identity

## Error Handling

The component provides comprehensive error handling:

- **NetworkMismatch**: Peers on different networks
- **NodeInfo**: Serialization/deserialization errors
- **Inbound/Outbound**: Network-level failures
- **Signature**: Cryptographic verification failures

## Testing

Comprehensive test suite covers:
- **Successful handshakes** between compatible peers
- **Network mismatch detection** and rejection
- **Serialization compatibility** with legacy formats
- **End-to-end integration** with libp2p swarms

## Security Considerations

- All handshake messages are cryptographically signed
- Network isolation prevents cross-network communication
- Size limits prevent DoS attacks
- Signature verification prevents impersonation

## Performance Characteristics

- **Low Latency**: Simple request-response exchange
- **Minimal Overhead**: Compact message format (typically <1KB)
- **Efficient Validation**: Fast network ID comparison
- **Async Processing**: Non-blocking I/O operations

## Compatibility

- **Protocol Version**: `/ssv/info/0.0.1`
- **Serialization**: JSON format for cross-language compatibility
- **Legacy Support**: Compatible with existing SSV network nodes
- **Future-Proof**: Extensible metadata structure

The handshake component is essential for maintaining network integrity and ensuring that Anchor nodes only communicate with compatible peers in the SSV ecosystem.