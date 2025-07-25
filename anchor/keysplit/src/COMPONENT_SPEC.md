# Keysplit Component Technical Specification

## API Specification

### Main Entry Point

```rust
pub fn run_keysplitter(
    keysplit: Keysplit,
    global_config: GlobalConfig,
) -> Result<(), KeysplitError>
```

**Purpose**: Main orchestration function that executes the complete key splitting workflow.

**Parameters**:
- `keysplit: Keysplit` - CLI configuration containing operation mode and parameters
- `global_config: GlobalConfig` - Global application configuration

**Returns**: `Result<(), KeysplitError>` - Success or detailed error information

### CLI Interface

#### Root Command Structure
```rust
pub struct Keysplit {
    #[clap(subcommand)]
    pub subcommand: KeygenSubcommands,
}

pub enum KeygenSubcommands {
    Onchain(Onchain),
    Manual(Manual),
}
```

#### Shared Options
```rust
pub struct SharedKeygenOptions {
    pub keystore_path: String,      // Path to validator keystore file
    pub password: String,           // Keystore decryption password
    pub owner: Address,             // EOA address owning the validator
    pub output_path: String,        // Output file path
    pub operators: OperatorIds,     // Target operator IDs
}
```

#### Manual Mode
```rust
pub struct Manual {
    pub shared: SharedKeygenOptions,
    pub nonce: u64,                     // Owner nonce value
    pub public_keys: Vec<Rsa<Public>>,  // Operator RSA public keys
}
```

#### Onchain Mode
```rust
pub struct Onchain {
    pub shared: SharedKeygenOptions,
    pub rpc: String,                    // Ethereum RPC endpoint
}
```

### Operator Configuration

#### Supported Cluster Sizes
- **4 operators**: 3-of-4 threshold
- **7 operators**: 5-of-7 threshold  
- **10 operators**: 7-of-10 threshold
- **13 operators**: 9-of-13 threshold

#### Threshold Calculation
```rust
let threshold = num_operators - ((num_operators - 1) / 3);
```

## Cryptographic Specifications

### BLS Threshold Signatures

**Library**: `bls_lagrange`
**Curve**: BLS12-381
**Threshold Scheme**: Shamir's Secret Sharing with Lagrange interpolation

#### Key Splitting Process
1. Parse validator BLS private key from keystore
2. Generate polynomial of degree `threshold - 1`
3. Evaluate polynomial at operator ID points
4. Distribute shares to corresponding operators

### RSA Encryption

**Key Size**: 2048-bit RSA keys
**Padding**: PKCS#1 v1.5 (OpenSSL default)
**Purpose**: Encrypt BLS key shares for individual operators

#### Encryption Process
1. Serialize BLS key share to bytes
2. Encode as hexadecimal string
3. Encrypt with operator's RSA public key
4. Store encrypted bytes in output structure

### Keystore Decryption

#### Supported KDF Algorithms

**PBKDF2**:
```rust
KdfparamsType::Pbkdf2 {
    c: u32,          // Iteration count
    dklen: u32,      // Derived key length
    prf: String,     // Pseudorandom function
    salt: Vec<u8>,   // Salt value
}
```

**Scrypt**:
```rust
KdfparamsType::Scrypt {
    dklen: u32,      // Derived key length
    n: u32,          // CPU/memory cost parameter
    p: u32,          // Parallelization parameter
    r: u32,          // Block size parameter
    salt: Vec<u8>,   // Salt value
}
```

#### Decryption Algorithm
1. Derive key using specified KDF
2. Verify MAC: `SHA256(derived_key[16:32] || ciphertext)`
3. Decrypt using AES-128-CTR with `derived_key[0:16]`
4. Deserialize BLS private key

## Data Structures

### Internal Types

```rust
struct ValidatorKeys {
    public_key: PublicKey,    // BLS public key
    secret_key: SecretKey,    // BLS private key
}

struct KeyShare {
    id: u64,                  // Operator ID
    public_key: Rsa<Public>,  // Operator RSA public key
    keyshare: SecretKey,      // BLS key share
}

struct EncryptedKeyShare {
    id: u64,                      // Operator ID
    public_key: Rsa<Public>,      // Operator RSA public key
    share_public_key: PublicKey,  // BLS public key for share
    encrypted_keyshare: Vec<u8>,  // Encrypted key share data
}
```

### Output Format

The tool generates JSON output containing:
- Encrypted key shares for each operator
- Validator public key information
- Metadata (owner, nonce, cluster configuration)
- Operator public keys and IDs

## Error Types

```rust
pub enum KeysplitError {
    Keystore(String),        // Keystore file errors
    InvalidKeyLen(String),   // Operator count mismatch
    InvalidOperator(String), // Unknown operator ID
    Password(String),        // Incorrect keystore password
    Output(String),          // File I/O errors
    Database(String),        // Database operation errors
    SplitFailure(String),    // BLS splitting errors
    Scrypt(String),          // Scrypt KDF errors
    Pbkdf2(String),          // PBKDF2 KDF errors
    Misc(String),            // General errors
}
```

## Database Schema (Onchain Mode)

### Operator Table
- `id: u64` - Operator identifier
- `public_key: Vec<u8>` - RSA public key bytes
- `active: bool` - Operator status

### Owner Table  
- `address: Address` - EOA address
- `nonce: u64` - Current nonce value

## Network Integration

### SSV Contract Interaction
- Monitors operator registration events
- Tracks owner nonce increments
- Maintains local cache of network state
- Supports various Ethereum networks via RPC

### Sync Process
1. Initialize SQLite database
2. Create SSV event syncer instance
3. Fetch historical events from contracts
4. Update local operator and owner data
5. Query required information for key splitting

## Security Considerations

### Threat Model
- **Malicious Operators**: Can't reconstruct validator key alone
- **Network Attacks**: Key shares encrypted per-operator
- **Local Compromise**: Keystore password required for access

### Best Practices
- Secure keystore password management
- Verify operator RSA public keys
- Use secure RPC endpoints for onchain mode
- Validate output data before distribution

### Cryptographic Assumptions
- RSA-2048 provides adequate encryption strength
- BLS12-381 curve security for threshold signatures
- KDF parameters provide sufficient iteration counts
- Random number generation is cryptographically secure

## Performance Characteristics

### Computational Complexity
- Key splitting: O(n) where n = number of operators
- RSA encryption: O(n) independent encryptions
- Database queries: O(log n) with proper indexing

### Memory Usage
- Keystore decryption: ~1KB working memory
- Key splitting: Linear in operator count
- Output generation: JSON serialization overhead

### Network Requirements (Onchain Mode)
- Ethereum RPC access for contract queries
- Historical event data synchronization
- Local SQLite database storage (~MB range)