# Network Component AI Documentation

## Overview

The `network` component is the core networking layer for the Anchor project, implementing a P2P networking stack built on libp2p. It provides decentralized peer discovery, gossip messaging, connection management, and secure communication for the SSV (Secret Shared Validator) network.

## Core Architecture

The network component is structured around several key modules:

### Main Components

- **Network** (`network.rs:1-600`): The main networking interface that orchestrates all networking functionality
- **Behaviour** (`behaviour.rs:36-50`): Combines multiple libp2p behaviors (identify, ping, gossipsub, discovery, peer management, handshake)
- **Config** (`config.rs:22-50`): Configuration management for network parameters, ports, and addresses
- **Discovery** (`discovery.rs:1-50`): Implements discv5 for peer discovery with subnet-aware queries
- **Transport** (`transport.rs:15-44`): Sets up TCP/QUIC transport layer with noise encryption and yamux multiplexing

### Specialized Modules

- **Handshake** (`handshake/`): Protocol for secure peer authentication and metadata exchange
- **Peer Manager** (`peer_manager/`): Advanced peer lifecycle management with connection tracking, heartbeats, and blocking
- **Scoring** (`scoring/`): Peer scoring system for network health and spam protection

## Key Functionality

### Peer Discovery
- Uses discv5 protocol for distributed peer discovery
- Implements subnet-aware peer queries for targeted discovery
- Maintains ENR (Ethereum Node Records) for peer metadata
- Supports both IPv4 and IPv6 addressing

### Gossip Communication
- Built on libp2p gossipsub for message propagation
- Implements topic-based message routing
- Includes peer scoring for spam protection
- Supports message validation and authentication

### Connection Management
- Manages peer connections with automatic reconnection
- Implements connection limits and quality control
- Provides peer blocking functionality
- Tracks connection health with ping/heartbeat mechanisms

### Security Features
- Noise protocol for encrypted communication
- Peer authentication through handshake protocol
- Message validation and spam protection
- Configurable peer scoring thresholds

## Integration Points

The network component integrates with:
- **Message Receiver**: For processing incoming gossip messages
- **Subnet Service**: For subnet-based peer organization
- **Task Executor**: For async task management
- **Types**: For SSV-specific data structures
- **Lighthouse Network**: For Ethereum networking utilities

## Configuration

Key configuration parameters include:
- Listen addresses and ports (TCP: 13001, QUIC: 13002, Discovery: 12001)
- ENR addresses for peer advertisement
- Network directory for key storage
- Connection limits and timeouts
- Scoring parameters and thresholds

## Error Handling

The component defines comprehensive error types:
- `NetworkError`: General networking errors
- `BehaviourError`: Behavior-specific errors
- `DiscoveryError`: Discovery-related errors
- Transport and connection errors

## Performance Considerations

- Supports both TCP and QUIC transports for optimal performance
- Implements efficient peer discovery with targeted queries
- Uses connection pooling and reuse
- Includes metrics and monitoring capabilities
- Configurable message size limits (5MB max)

## Dependencies

Major dependencies include:
- `libp2p`: Core P2P networking library
- `discv5`: Ethereum discovery protocol
- `gossipsub`: Publish-subscribe messaging
- `lighthouse_network`: Ethereum networking utilities
- `tokio`: Async runtime