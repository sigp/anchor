# Anchor Client Component

## Overview

The Anchor Client is the main orchestration component of the SSV (Secret Shared Validator) node, responsible for coordinating all validator operations and managing interactions with the Ethereum consensus layer. It serves as the central entry point that integrates various subsystems including networking, QBFT consensus, signature collection, and validator duties management.

## Architecture

The client follows a modular architecture where each major functionality is encapsulated in separate services that communicate through well-defined interfaces:

```
┌─────────────────────────────────────────────────────────────────┐
│                        Anchor Client                            │
├─────────────────────────────────────────────────────────────────┤
│  ┌─────────────────┐  ┌─────────────────┐  ┌─────────────────┐  │
│  │   CLI Interface │  │   Configuration │  │   Key Management│  │
│  └─────────────────┘  └─────────────────┘  └─────────────────┘  │
├─────────────────────────────────────────────────────────────────┤
│  ┌─────────────────┐  ┌─────────────────┐  ┌─────────────────┐  │
│  │  HTTP API       │  │  HTTP Metrics   │  │   Database      │  │
│  └─────────────────┘  └─────────────────┘  └─────────────────┘  │
├─────────────────────────────────────────────────────────────────┤
│  ┌─────────────────┐  ┌─────────────────┐  ┌─────────────────┐  │
│  │   Networking    │  │   Message       │  │   QBFT Manager  │  │
│  │     (P2P)       │  │   Processing    │  │                 │  │
│  └─────────────────┘  └─────────────────┘  └─────────────────┘  │
├─────────────────────────────────────────────────────────────────┤
│  ┌─────────────────┐  ┌─────────────────┐  ┌─────────────────┐  │
│  │  Signature      │  │   Duties        │  │   Validator     │  │
│  │  Collector      │  │   Tracker       │  │   Services      │  │
│  └─────────────────┘  └─────────────────┘  └─────────────────┘  │
├─────────────────────────────────────────────────────────────────┤  
│  ┌─────────────────┐  ┌─────────────────┐  ┌─────────────────┐  │
│  │  Beacon Node    │  │  Execution Node │  │   Slot Clock    │  │
│  │   Interface     │  │   Interface     │  │                 │  │
│  └─────────────────┘  └─────────────────┘  └─────────────────┘  │
└─────────────────────────────────────────────────────────────────┘
```

## Core Components

### 1. Client Orchestration (`lib.rs`)
- **Main Entry Point**: The `Client::run()` method serves as the primary orchestration point
- **Service Initialization**: Manages the startup sequence of all subsystems
- **Resource Management**: Handles shared resources like databases, network connections, and thread pools
- **Graceful Shutdown**: Coordinates shutdown procedures across all services

### 2. CLI Interface (`cli.rs`)
- **Command Line Parsing**: Handles all command-line arguments and flags using clap
- **Configuration Validation**: Ensures CLI parameters are valid and compatible
- **User Experience**: Provides comprehensive help text and error messages
- **Network Configuration**: Manages network addresses, ports, and discovery settings

### 3. Configuration Management (`config.rs`)
- **Configuration Parsing**: Converts CLI arguments into structured configuration objects
- **Default Values**: Provides sensible defaults for all configuration options
- **URL Parsing**: Handles beacon node, execution node, and websocket endpoint configurations
- **Network Address Resolution**: Manages IPv4/IPv6 dual-stack networking configurations

### 4. Key Management (`key.rs`)
- **Operator Key Loading**: Reads and validates operator private keys
- **Key Generation**: Creates new keys when none exist
- **Encryption Support**: Handles both encrypted (.json) and unencrypted (.txt) key formats
- **Security**: Implements secure key handling practices

### 5. Notification System (`notifier.rs`)
- **Event Logging**: Provides structured logging for important system events
- **Metrics Collection**: Gathers and reports operational metrics
- **Status Updates**: Monitors and reports validator and network status
- **Alert Generation**: Triggers alerts for critical conditions

## Key Responsibilities

### Service Orchestration
- Initializes and manages the lifecycle of all subsystem services
- Coordinates startup dependencies ensuring proper initialization order
- Manages shared state and communication channels between services
- Handles graceful shutdown procedures

### Network Management
- Establishes P2P network connections with other SSV nodes
- Manages beacon node and execution node connectivity with fallback support
- Handles network configuration including ports, addresses, and discovery
- Implements connection health monitoring and automatic failover

### Validator Operations
- Coordinates validator duties (attestations, proposals, sync committee participation)
- Manages validator registration and metadata updates
- Handles voluntary exits and slashing protection
- Integrates with builder networks for MEV-boost functionality

### Data Persistence
- Manages the SQLite database for persistent state storage
- Handles slashing protection database operations
- Maintains operator and validator information
- Persists network state and peer information

### Security & Compliance
- Implements slashing protection to prevent validator penalties
- Manages secure key storage and usage
- Validates all incoming messages and consensus operations
- Maintains audit trails for all validator actions

## Integration Points

### External Systems
- **Beacon Node API**: RESTful HTTP interface for consensus layer operations
- **Execution Node**: JSON-RPC interface for execution layer data
- **SSV Network**: P2P networking with other SSV operators
- **Builder Networks**: MEV-boost integration for block proposals

### Internal Services
- **Database**: Persistent storage for all operational data  
- **QBFT Manager**: Byzantine fault-tolerant consensus protocol
- **Signature Collector**: Aggregates partial signatures from operators
- **Message Validator**: Validates all network messages
- **Duties Tracker**: Monitors and schedules validator duties
- **Network Service**: Manages P2P communications

## Error Handling & Resilience

### Fault Tolerance
- Implements comprehensive error handling with detailed logging
- Provides automatic recovery mechanisms for transient failures
- Maintains service isolation to prevent cascading failures
- Includes circuit breaker patterns for external dependencies

### Monitoring & Observability
- Exposes Prometheus metrics for operational monitoring
- Provides HTTP API for runtime introspection
- Implements structured logging with configurable levels
- Tracks performance metrics and resource utilization

## Configuration Management

The client supports extensive configuration through CLI arguments covering:
- Network settings (addresses, ports, discovery)
- External API endpoints (beacon nodes, execution nodes)
- Performance tuning (timeouts, worker threads, queue sizes)
- Security options (TLS certificates, slashing protection)
- Monitoring and metrics (HTTP API, Prometheus metrics)
- Validator-specific settings (gas limits, builder preferences)