# Signature Collector - AI Documentation

## Overview
The `signature_collector` component is a critical part of the SSV (Secret Shared Validator) system that manages the collection and aggregation of partial BLS signatures from multiple operators to reconstruct complete signatures. It implements a distributed threshold signature scheme where a minimum number of partial signatures (threshold) from different operators can be combined to create a valid signature.

## Architecture

### Core Components

1. **SignatureCollectorManager**: The main orchestrator that manages signature collection instances and coordinates with other system components.
2. **SignatureCollector**: Individual instances that collect partial signatures for specific signing roots and validator indices.
3. **CommitteeSignatures**: Manages signature collection for entire committees, coordinating multiple validator signatures.

### Key Data Structures

- `SignatureCollectorManager`: Central manager containing:
  - `processor`: Message processing interface
  - `operator_id`: Local operator identity
  - `domain`: Network domain for message identification
  - `message_sender`: Network communication interface
  - `signature_collectors`: Map of active collector instances
  - `committee_signatures`: Committee-level signature aggregation

- `SignatureMetadata`: Contains signature context:
  - `kind`: Type of partial signature
  - `role`: Validator role in consensus
  - `threshold`: Minimum signatures needed
  - `slot`: Time slot for signature validity
  - `committee_id`: Committee identifier

### Workflow

1. **Signature Request**: Components request signatures via `sign_and_collect()`
2. **Instance Management**: Manager creates or retrieves collector instances
3. **Partial Signing**: Local operator creates partial signature using BLS key share
4. **Collection**: Instance collects partial signatures from network peers
5. **Threshold Check**: Once threshold is met, signatures are combined using Lagrange interpolation
6. **Reconstruction**: Full signature is reconstructed and returned to requesters
7. **Cleanup**: Old instances are periodically cleaned up to prevent memory leaks

### Message Flow

```
sign_and_collect() → get_or_spawn() → signature_collector()
                                   ↓
receive_partial_signature() → CollectorMessage → combine_signatures()
                                              ↓
                                         Full Signature
```

## Key Features

- **Threshold Signatures**: Implements BLS threshold signature scheme with configurable thresholds
- **Distributed Collection**: Collects signatures from multiple network operators
- **Committee Support**: Handles both single validator and committee-wide signature collection
- **Memory Management**: Automatic cleanup of old collector instances
- **Error Handling**: Comprehensive error handling for network, cryptographic, and consensus failures
- **Concurrent Processing**: Async/await based concurrent signature collection

## Dependencies

- `bls_lagrange`: BLS signature operations and Lagrange interpolation
- `dashmap`: Concurrent hash maps for thread-safe collections
- `database`: Operator identity management
- `message_sender`: Network message transmission
- `processor`: Task queue and execution management
- `ssv_types`: SSV protocol type definitions
- `tokio`: Async runtime and synchronization primitives

## Error Handling

The component defines `CollectionError` enum covering:
- Queue management errors (full/closed queues)
- Network timeouts
- Cryptographic failures
- Invalid signatures
- Operator identification issues

## Cleanup and Lifecycle

- Collector instances are retained for `SIGNATURE_COLLECTOR_RETAIN_SLOTS` (1 slot)
- Background cleaner task runs on slot boundaries
- Automatic cleanup prevents unbounded memory growth
- Graceful shutdown when processor is closed

This component is essential for the SSV protocol's distributed validator operation, enabling secure signature aggregation across multiple operators while maintaining Byzantine fault tolerance.