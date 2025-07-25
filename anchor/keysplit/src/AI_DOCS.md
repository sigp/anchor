# Keysplit AI Documentation

## Component Overview

The **keysplit** component is a command-line tool for splitting validator keys among multiple SSV (Secret Shared Validator) operators using threshold cryptography. It securely distributes a validator's private key across multiple operators while maintaining the ability to reconstruct signatures through threshold schemes.

## Architecture

### Core Modules

- **`lib.rs`** - Main entry point and orchestration of key splitting workflow
- **`split.rs`** - Core splitting logic for manual and onchain modes
- **`crypto.rs`** - Cryptographic operations (key extraction, splitting, encryption)
- **`cli.rs`** - Command-line interface definitions and argument parsing
- **`keystore.rs`** - Keystore file parsing and validation
- **`output.rs`** - Output data structures and serialization
- **`error.rs`** - Error types and handling
- **`util.rs`** - Utility functions

### Key Data Structures

#### `KeyShare`
```rust
struct KeyShare {
    id: u64,                    // Operator ID
    public_key: Rsa<Public>,    // Operator's RSA public key
    keyshare: SecretKey,        // BLS secret key share
}
```

#### `EncryptedKeyShare`
```rust
struct EncryptedKeyShare {
    id: u64,                        // Operator ID
    public_key: Rsa<Public>,        // Operator's RSA public key
    share_public_key: PublicKey,    // BLS public key for this share
    encrypted_keyshare: Vec<u8>,    // RSA-encrypted secret key share
}
```

## Workflow

### 1. Key Extraction (`crypto.rs:extract_key`)
- Reads and parses validator keystore file
- Derives decryption key using PBKDF2 or Scrypt
- Validates MAC and decrypts validator private key

### 2. Key Splitting (`crypto.rs:split_keys`)
- Uses BLS Lagrange interpolation to split the validator key
- Creates threshold scheme: `threshold = num_operators - ((num_operators - 1) / 3)`
- Generates individual key shares for each operator

### 3. Encryption (`crypto.rs:encrypt_keyshares`)
- Encrypts each key share with the corresponding operator's RSA public key
- Ensures only the intended operator can decrypt their share

### 4. Output Generation (`output.rs`)
- Constructs structured output containing all encrypted key shares
- Serializes to JSON format for distribution

## Operation Modes

### Manual Mode
- Requires explicit input of operator RSA public keys
- User provides nonce value manually
- Direct control over all parameters

### Onchain Mode
- Fetches operator data from SSV network smart contracts
- Automatically determines nonce from owner's transaction history
- Reduces human error through automated data retrieval

## Security Features

### Threshold Cryptography
- Uses BLS threshold signatures with Byzantine fault tolerance
- Supports 4, 7, 10, or 13 operator configurations
- Threshold calculation ensures network remains operational with up to 1/3 offline operators

### Encryption Layers
1. **Keystore Encryption**: Original validator key protected by password-derived key
2. **Share Encryption**: Each key share encrypted with operator's unique RSA public key
3. **MAC Validation**: Integrity checking throughout the process

### Key Isolation
- Each operator receives only their encrypted key share
- No single operator can reconstruct the original validator key
- Requires threshold number of operators to generate valid signatures

## Dependencies

### Cryptographic Libraries
- `bls_lagrange` - BLS threshold cryptography
- `openssl` - RSA encryption and key operations
- `aes`, `ctr` - AES-CTR encryption for keystore decryption
- `pbkdf2`, `scrypt` - Key derivation functions
- `sha2` - Hash functions for MAC validation

### Network Integration
- `database` - SQLite database for operator data storage
- `eth` - Ethereum integration for onchain data fetching
- `alloy` - Ethereum client library

## Error Handling

The component uses a comprehensive error system (`KeysplitError`) covering:
- Keystore parsing and decryption errors
- Invalid operator configurations
- Cryptographic operation failures
- Network and database errors
- File I/O and serialization errors

## Integration Points

### SSV Network
- Integrates with SSV smart contracts for operator discovery
- Fetches operator public keys and owner nonces from blockchain
- Maintains local database of network state

### Validator Infrastructure
- Processes standard EIP-2335 keystore files
- Outputs data compatible with SSV operator nodes
- Supports multiple validator key formats and encryption schemes