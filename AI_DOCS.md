# Anchor SSV - AI Documentation

## Project Overview

Anchor is an open-source implementation of the Secret Shared Validator (SSV) protocol written in Rust. It enables distributed validator operations across multiple operators to enhance Ethereum validator resilience and decentralization.

### Key Characteristics
- **Language**: Rust
- **Domain**: Ethereum Consensus Layer, Distributed Validator Technology
- **Architecture**: Modular, service-oriented design
- **Consensus**: QBFT (Istanbul BFT variant)
- **Network**: libp2p-based P2P networking

## Component Architecture

### Core Components

#### 1. Client (`anchor/client/`)
- **Purpose**: Main orchestration component
- **Key Files**: `lib.rs`, `cli.rs`, `config.rs`, `key.rs`, `notifier.rs`
- **Documentation**: [Client AI Docs](anchor/client/src/AI_DOCS.md)

#### 2. Network Layer (`anchor/network/`)
- **Purpose**: P2P networking and peer management
- **Key Modules**: 
  - Handshake protocol ([docs](anchor/network/src/handshake/AI_DOCS.md))
  - Peer management ([docs](anchor/network/src/peer_manager/AI_DOCS.md))
  - Scoring system ([docs](anchor/network/src/scoring/AI_DOCS.md))
- **Documentation**: [Network AI Docs](anchor/network/src/AI_DOCS.md)

#### 3. Consensus (`anchor/qbft_manager/`, `anchor/common/qbft/`)
- **Purpose**: Byzantine fault-tolerant consensus protocol
- **Documentation**: 
  - [QBFT Manager AI Docs](anchor/qbft_manager/src/AI_DOCS.md)
  - [QBFT Common AI Docs](anchor/common/qbft/AI_DOCS.md)

#### 4. Message Processing
- **Message Receiver** ([docs](anchor/message_receiver/src/AI_DOCS.md))
- **Message Sender** ([docs](anchor/message_sender/src/AI_DOCS.md))
- **Message Validator** ([docs](anchor/message_validator/src/AI_DOCS.md))
- **Processor** ([docs](anchor/processor/src/AI_DOCS.md))

#### 5. Signature Operations
- **Signature Collector** ([docs](anchor/signature_collector/src/AI_DOCS.md))
- **BLS Lagrange** ([docs](anchor/common/bls_lagrange/))

#### 6. Validator Management
- **Validator Store** ([docs](anchor/validator_store/src/AI_DOCS.md))
- **Duties Tracker** ([docs](anchor/duties_tracker/src/AI_DOCS.md))
- **Subnet Service** ([docs](anchor/subnet_service/src/AI_DOCS.md))

#### 7. Ethereum Integration
- **Eth Module** ([docs](anchor/eth/src/AI_DOCS.md))
- **SSV Types** ([docs](anchor/common/ssv_types/AI_DOCS.md))

#### 8. Data & Storage
- **Database** ([docs](anchor/database/src/AI_DOCS.md))
- **Key Generation** ([docs](anchor/keygen/src/AI_DOCS.md))
- **Key Splitting** ([docs](anchor/keysplit/src/AI_DOCS.md))

#### 9. Infrastructure
- **HTTP API** ([docs](anchor/http_api/src/AI_DOCS.md))
- **HTTP Metrics** ([docs](anchor/http_metrics/src/AI_DOCS.md))
- **Logging** ([docs](anchor/logging/src/AI_DOCS.md))

## Data Flow Overview

```
┌─────────────────┐    ┌─────────────────┐    ┌─────────────────┐
│   Beacon Node   │────│  Anchor Client  │────│   SSV Network   │
│   (Ethereum)    │    │  (Orchestrator) │    │   (P2P Peers)   │
└─────────────────┘    └─────────────────┘    └─────────────────┘
                              │
                              ▼
                    ┌─────────────────┐
                    │  QBFT Consensus │
                    │   (Byzantine    │
                    │  Fault Tolerant)│
                    └─────────────────┘
                              │
                              ▼
                    ┌─────────────────┐
                    │ Signature       │
                    │ Aggregation     │
                    └─────────────────┘
```

## Development Guidelines

### Code Standards
- Follow Rust best practices and idioms
- Use `thiserror` for error handling
- Implement comprehensive testing
- Follow existing logging patterns with `tracing`
- Use workspace dependencies for consistency

### Testing Strategy
- Unit tests for individual components
- Integration tests for cross-component functionality
- Property-based testing where applicable
- Network simulation tests for distributed scenarios

### Security Considerations
- Slashing protection is critical - never compromise validator safety
- Secure handling of cryptographic keys
- Message validation and authentication
- Network security and peer verification

## Key External Dependencies

- **Lighthouse**: Ethereum consensus client dependencies
- **libp2p**: Peer-to-peer networking
- **tokio**: Async runtime
- **alloy**: Ethereum types and utilities
- **blst/blstrs**: BLS signature library
- **rusqlite**: SQLite database interface

## Monitoring & Observability

- Prometheus metrics available via HTTP metrics endpoint
- Structured logging with configurable levels
- Health checks and service status monitoring
- Performance metrics and resource tracking

## Build & Development

```bash
# Build entire workspace
cargo build --release

# Run tests
cargo test

# Run specific component tests
cargo test -p database

# Generate documentation
cargo doc --open
```

## AI Documentation Structure

Each component includes:
- `AI_DOCS.md`: Comprehensive technical documentation
- `COMPONENT_SPEC.md`: Formal specification and interfaces
- `USAGE_EXAMPLES.md`: Practical usage examples and patterns

This documentation framework enables AI assistants to:
- Understand component relationships and dependencies
- Identify appropriate patterns for new features
- Maintain consistency with existing architecture
- Implement secure and compliant code changes

## Related Resources

- [Anchor Book](https://anchor-book.sigmaprime.io): User documentation
- [SSV Protocol](https://docs.ssv.network/): Protocol specification
- [Lighthouse](https://lighthouse-book.sigmaprime.io/): Ethereum consensus client