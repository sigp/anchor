# SSV Types - AI Documentation

## Overview

The `ssv_types` crate provides core type definitions and data structures for Secret Shared Validator (SSV) operations within the Anchor consensus client. This library implements the fundamental types needed for distributed validator technology, including cluster management, operator coordination, consensus messaging, and cryptographic key sharing.

## Key Components

### Core Entity Types

#### 1. **Cluster Management** (`cluster.rs`)
- **`Cluster`**: Represents a group of operators acting on behalf of validators
  - Contains cluster ID, owner address, fee recipient, liquidation status
  - Manages cluster members (operators) via `IndexSet<OperatorId>`
  - Provides Byzantine fault tolerance calculations (`get_f()` method)
- **`ClusterId`**: 32-byte unique identifier for clusters
- **`ValidatorMetadata`**: General metadata about validators including public key, cluster assignment, and graffiti

#### 2. **Operator Management** (`operator.rs`)
- **`Operator`**: Client responsible for network health maintenance
  - Contains RSA public key for cryptographic operations
  - Includes owner address and unique operator ID
- **`OperatorId`**: Unique 64-bit identifier for operators

#### 3. **Committee System** (`committee.rs`)
- **`CommitteeInfo`**: Structure holding committee members and validator indices
- **`CommitteeId`**: 32-byte hash-based identifier derived from sorted operator IDs
- Implements deterministic committee identification via SHA256 hashing

### Messaging Infrastructure

#### 4. **Message Types** (`message.rs`)
- **`SSVMessage`**: Base message structure with type, ID, and data payload
  - Supports consensus messages and partial signature messages
  - Implements size validation based on message type
- **`SignedSSVMessage`**: Cryptographically signed SSV message
  - Contains RSA signatures (256 bytes each, max 13)
  - Includes operator IDs and full message data
  - Implements strict validation rules for Byzantine fault tolerance
- **`MsgType`**: Enum distinguishing consensus vs partial signature messages

#### 5. **Consensus Protocol** (`consensus.rs`)
- **`QbftMessage`**: QBFT (Quorum-based Byzantine Fault Tolerant) consensus messages
  - Includes message type (Proposal, Prepare, Commit, RoundChange)
  - Contains height, round, identifier, and justification data
- **`ValidatorConsensusData`**: Consensus data for validator duties
- **`BeaconVote`**: Attestation voting data with block roots and checkpoints
- **`ValidatorDuty`**: Detailed duty assignment for validators

### Cryptographic Components

#### 6. **Key Sharing** (`share.rs`)
- **`Share`**: Represents one of N shares of a split validator key
  - Contains validator public key, operator assignment, and cluster ID
  - Includes encrypted private key share (256 bytes)
  - Enables distributed key management for validator security

#### 7. **Message Identification** (`msgid.rs`)
- **`MessageId`**: 56-byte fixed-size message identifier
- Enables unique message tracking across the distributed system

## Architecture Principles

### 1. **Byzantine Fault Tolerance**
The system is designed around the principle that up to `f` operators can be faulty in a cluster of `3f+1` operators. This is reflected in:
- Cluster size validation
- Signature aggregation requirements
- Message validation constraints

### 2. **Cryptographic Security**
- Uses RSA signatures for operator authentication
- Implements AES encryption for private key shares
- Provides deterministic hashing for committee identification

### 3. **SSZ Serialization**
All data structures implement Simple Serialize (SSZ) encoding for:
- Deterministic serialization
- Compatibility with Ethereum consensus layer
- Efficient network transmission

### 4. **Type Safety**
- Extensive use of newtype patterns (e.g., `OperatorId`, `ClusterId`)
- Compile-time guarantees for identifier uniqueness
- Validation at construction time for all complex types

## Integration Points

### External Dependencies
- **`types`**: Ethereum consensus types (Slot, Epoch, PublicKeyBytes, etc.)
- **`ethereum_ssz`**: SSZ serialization framework
- **`openssl`**: RSA cryptographic operations
- **`indexmap`**: Ordered collections for deterministic behavior

### Internal Coordination
- Operators coordinate through signed message passing
- Clusters maintain operator membership via committee systems
- Validators distribute trust across multiple operators through key sharing

## Security Model

The SSV types implement a multi-layered security approach:

1. **Identity Verification**: RSA signatures ensure operator authenticity
2. **Byzantine Tolerance**: Mathematical guarantees against up to `f` faulty operators
3. **Key Distribution**: Validator keys are split across multiple operators
4. **Message Integrity**: All communications are cryptographically signed and validated

This architecture enables secure, distributed validator operations while maintaining compatibility with Ethereum's consensus requirements.