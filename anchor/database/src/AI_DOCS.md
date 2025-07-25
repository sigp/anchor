# Database Component AI Documentation

## Overview

The `database` component is a comprehensive SQLite-based data management system for the Anchor SSV (Secret Shared Validators) network. It provides persistent storage and in-memory caching for operators, clusters, validators, and cryptographic shares in a distributed validator system.

## Core Purpose

This component manages the complete lifecycle of:
- **Operators**: Network participants who operate validator nodes
- **Clusters**: Groups of operators managing shared validator keys  
- **Validators**: Ethereum validators with distributed key shares
- **Shares**: Encrypted cryptographic key fragments distributed among operators

## Key Architecture Components

### NetworkDatabase
The main database interface that combines:
- SQLite persistence layer with connection pooling
- In-memory state management using `MultiIndexMap` for fast lookups
- Watch-based state notifications for reactive updates
- Transactional operations ensuring data consistency

### State Management
- **MultiState**: Multi-indexed maps for complex entity relationships
- **SingleState**: Simple key-value storage for metadata and configuration
- **NetworkState**: Combined state container with accessor methods

### Multi-Index Maps
Custom data structures enabling efficient lookups by multiple keys:
- `ShareMultiIndexMap`: Shares indexed by validator public key, cluster ID, and owner
- `MetadataMultiIndexMap`: Validator metadata with same indexing scheme  
- `ClusterMultiIndexMap`: Clusters indexed by ID, validator key, and owner

## Main Operations

### Database Management
- Initialize new databases or connect to existing ones
- Connection pooling with configurable timeouts
- Block number tracking for event synchronization

### Operator Management
- Register/remove operators with RSA public keys
- Operator existence verification and lookup

### Cluster Operations  
- Create clusters when first validator is added
- Update cluster status (active/liquidated)
- Remove clusters when last validator is deleted
- Owner nonce management for transaction ordering

### Validator Management
- Add/remove validators from clusters
- Update validator metadata (graffiti, indices)
- Fee recipient management per cluster owner

### Share Management
- Store encrypted key shares for validators
- Associate shares with specific operators
- Key retrieval for cryptographic operations

## Data Relationships

```
Owners (1:N) -> Clusters (1:N) -> Validators (1:N) -> Shares (N:M) -> Operators
```

- Owners can have multiple clusters
- Clusters contain multiple validators
- Each validator has shares distributed across multiple operators
- Operators can participate in multiple clusters

## Key Features

- **Transactional Integrity**: All operations use database transactions
- **In-Memory Performance**: Critical data cached for fast access
- **Reactive Updates**: Watch-based notifications for state changes
- **Multi-Index Lookups**: Efficient queries by different entity relationships
- **Cascade Operations**: Proper cleanup when entities are removed
- **Operator Identity**: Support for both RSA public keys and operator IDs

## Error Handling

Comprehensive error types covering:
- Database connection and pool issues
- SQL operation failures
- Entity not found conditions
- Duplicate entity prevention
- I/O errors during file operations

## Dependencies

- `rusqlite`: SQLite database interface
- `r2d2`: Connection pooling
- `ssv_types`: Core SSV data types
- `openssl`: RSA key management
- `tokio`: Async runtime and synchronization primitives

This database component serves as the foundation for distributed validator key management in the SSV network, providing reliable persistence and efficient access patterns for complex multi-party cryptographic operations.