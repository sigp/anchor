# BLS Lagrange Component

## Overview
The `bls_lagrange` component implements BLS (Boneh-Lynn-Shacham) threshold signature schemes using Lagrange interpolation for secret sharing. This component is part of the Anchor project by Sigma Prime and provides cryptographic primitives for distributed signature generation.

## Location
- **Path**: `anchor/common/bls_lagrange/`
- **Type**: Rust library crate
- **Version**: 0.1.0

## Purpose
This component enables:
1. **Secret Key Splitting**: Split a master BLS secret key into multiple shares using Shamir's Secret Sharing
2. **Threshold Signatures**: Combine partial signatures from a threshold number of participants to reconstruct the original signature
3. **Lagrange Interpolation**: Use mathematical interpolation to recover secrets from partial shares

## Key Features

### Dual Implementation Support
- **BLST Backend** (`blst.rs`): High-performance implementation using the BLST library with manual Lagrange interpolation
- **Blsful Backend** (`blsful.rs`): Alternative implementation using blstrs_plus and vsss-rs libraries

### Configuration Features
- `default = ["blst_single_thread"]`: Default to single-threaded BLST
- `blsful`: Enable blsful backend with vsss-rs support
- `blst`: Enable BLST backend
- `blst_single_thread`: Single-threaded BLST variant

### Core Functionality
- **Key Splitting**: `split()` and `split_with_rng()` functions to create threshold shares
- **Signature Combination**: `combine_signatures()` to reconstruct signatures from partial shares
- **KeyId Management**: Secure handling of participant identifiers with zero-protection

## API Structure

### Main Types
- `KeyId`: Represents a participant identifier (non-zero u64 with cryptographic scalar representation)
- `Error`: Comprehensive error handling for various failure modes

### Key Functions
```rust
// Split a secret key into threshold shares
pub fn split(key: &SecretKey, threshold: u64, ids: impl IntoIterator<Item = KeyId>) -> Result<Vec<(KeyId, SecretKey)>, Error>

// Combine partial signatures to reconstruct the original signature
pub fn combine_signatures(signatures: &[Signature], ids: &[KeyId]) -> Result<Signature, Error>
```

### Error Types
- `InternalError`: Cryptographic operation failures
- `InvalidThreshold`: Threshold parameter validation errors
- `LessThanTwoSignatures`: Insufficient signatures for combination
- `NotOneIdPerSignature`: Mismatch between signatures and IDs
- `ZeroId`: Invalid zero participant ID
- `ZeroKey`: Invalid zero secret key
- `RepeatedId`: Duplicate participant IDs
- `InvalidSignature`: Signature validation failures

## Implementation Details

### BLST Backend (`blst.rs`)
- Direct implementation of Lagrange interpolation algorithm
- Manual polynomial evaluation using Horner's method
- Optimized scalar arithmetic using unsafe BLST operations
- Single-threaded and multi-threaded variants available
- Performance-focused with manual memory management

### Blsful Backend (`blsful.rs`)
- Uses `vsss-rs` crate for secret sharing primitives
- Higher-level abstractions with built-in error handling
- Automatic participant ID generation and validation
- Type-safe scalar field operations using `blstrs_plus`

## Security Considerations
- All secret keys are automatically zeroized on drop
- Participant IDs must be non-zero to prevent secret exposure
- Threshold must be ≥ 2 for meaningful security
- Duplicate participant IDs are detected and rejected
- Invalid signatures are validated before processing

## Testing
Comprehensive test suite includes:
- Basic threshold signature functionality
- Edge case validation (zero keys, invalid thresholds, etc.)
- Error condition testing
- Performance benchmarking
- Cryptographic correctness verification

## Dependencies
- `bls`: Workspace BLS signature implementation
- `blst`: Optional BLST cryptographic library
- `blstrs_plus`: Optional alternative BLS implementation
- `vsss-rs`: Optional Verifiable Secret Sharing library
- `rand`: Cryptographic random number generation
- `zeroize`: Secure memory clearing

## Integration
This component is used within the Anchor ecosystem for:
- Distributed validator key management
- Threshold signature schemes in consensus protocols
- Multi-party cryptographic operations
- Secure secret sharing for validator operations

## Performance Notes
- BLST backend optimized for high-performance scenarios
- Single-threaded variant available for embedded or constrained environments
- Benchmarking utilities included for performance validation
- Memory-efficient implementation with automatic cleanup