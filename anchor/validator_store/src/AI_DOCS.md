# Anchor Validator Store - AI Documentation

## Overview

The `anchor_validator_store` component is a specialized validator management system for the Anchor SSV (Secret Shared Validator) protocol. It manages validator keys, handles distributed signature collection, coordinates QBFT consensus for validator duties, and integrates with Ethereum validator operations.

## Architecture

### Core Components

1. **AnchorValidatorStore** (`lib.rs:97-117`)
   - Main validator store implementation
   - Manages validator lifecycle and metadata
   - Coordinates signature collection and consensus
   - Implements the `ValidatorStore` trait for validator operations

2. **MetadataService** (`metadata_service.rs:14-21`)
   - Updates slot metadata for validators
   - Fetches attestation data from beacon nodes
   - Manages sync committee duties and aggregator assignments

3. **Metrics Module** (`metrics.rs`)
   - Provides performance monitoring for consensus operations
   - Tracks signing times and success rates

### Key Data Structures

- **InitializedValidator** (`lib.rs:91-95`): Contains cluster info, metadata, and decrypted key share
- **SlotMetadata** (`lib.rs:676-688`): Slot-specific validator duties and beacon vote data
- **ContributionWaiter** (`lib.rs:690-716`): Synchronization for multi-subnet sync aggregators

## Core Functionality

### Validator Management
- Dynamic validator loading from database state (`load_validators:172`)
- Key share decryption using RSA private keys (`get_share_from_state:228`)
- Validator registration with slashing protection (`add_validator:277`)

### Consensus Integration
- QBFT consensus for block proposals (`decide_abstract_block:403`)
- Committee-level consensus for attestations and sync duties
- Timeout handling and performance monitoring

### Signature Collection
- Distributed signature collection via SignatureCollectorManager
- Support for various signature types (blocks, attestations, aggregates)
- Threshold signature aggregation based on cluster configuration

### Slashing Protection
- Integration with SlashingDatabase for safety checks
- Periodic pruning of historical data (`prune_slashing_protection_db:1465`)
- Configurable slashing protection disable for testing

## Dependencies

- **qbft_manager**: QBFT consensus coordination
- **signature_collector**: Distributed signature collection
- **database**: Validator metadata and cluster storage  
- **slashing_protection**: Validator safety checks
- **validator_store**: Base validator store trait
- **ssv_types**: SSV protocol types and structures

## Integration Points

- Implements `ValidatorStore` trait for validator client integration
- Integrates with beacon node fallback for chain data
- Coordinates with task executor for background services
- Uses slot clock for timing-sensitive operations

## Configuration

Key configuration parameters:
- `disable_slashing_protection`: Safety override for testing
- `gas_limit`: Block proposal gas limit
- `builder_proposals`: MEV-boost integration flag
- `builder_boost_factor`: Builder selection preference

## Thread Safety

The component uses concurrent data structures (`DashMap`) and atomic operations for thread-safe validator management across multiple background tasks.