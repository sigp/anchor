# Keygen Component - Technical Specification

## Component Metadata

- **Name**: keygen
- **Version**: 0.2.0
- **Language**: Rust
- **Edition**: 2021 (workspace inherited)
- **Authors**: Sigma Prime <contact@sigmaprime.io>

## API Specification

### Public Functions

#### `run_keygen(keygen: Keygen, data_dir: &Path) -> Result<Rsa<Private>, KeygenError>`

**Purpose**: Main key generation function that creates RSA key pairs and stores them securely.

**Parameters**:
- `keygen: Keygen` - Configuration struct containing CLI options
- `data_dir: &Path` - Directory path where keys will be stored

**Returns**: 
- `Ok(Rsa<Private>)` - Generated RSA private key on success
- `Err(KeygenError)` - Specific error type on failure

**Behavior**:
1. Generates 2048-bit RSA private key using OpenSSL
2. Converts public key to SSV protocol format
3. Determines output file paths based on encryption setting:
   - Encrypted: `encrypted_private_key.json`
   - Unencrypted: `private_key.txt`
   - Public key: `public_key.txt` (always)
4. Checks for existing files unless `force` flag is set
5. Handles password input for encryption if required
6. Writes keys to filesystem with appropriate security measures

#### `read_password_from_user(confirm: bool) -> Result<Zeroizing<String>, KeygenError>`

**Purpose**: Securely reads password from user input with optional confirmation.

**Parameters**:
- `confirm: bool` - Whether to prompt for password confirmation

**Returns**:
- `Ok(Zeroizing<String>)` - Securely allocated password string
- `Err(KeygenError)` - Error if password reading fails

**Behavior**:
1. Prompts user for password (masked input)
2. If `confirm` is true, prompts for password confirmation
3. Validates passwords match if confirmation is enabled
4. Returns password in zero-on-drop container for security

### Data Structures

#### `Keygen` Struct

```rust
pub struct Keygen {
    pub force: bool,
    pub encrypt: bool,
    pub password_file: Option<PathBuf>,
}
```

**Fields**:
- `force: bool` - Forces overwrite of existing key files (default: false)
- `encrypt: bool` - Enables password-based encryption of private key
- `password_file: Option<PathBuf>` - Optional path to file containing password (requires `encrypt`)

**Clap Attributes**:
- Derives from `clap::Parser` for CLI argument parsing
- Includes help text and validation rules
- `password_file` requires `encrypt` to be enabled

#### `KeygenError` Enum

Comprehensive error handling with specific error types:

```rust
pub enum KeygenError {
    Generate(ErrorStack),           // OpenSSL key generation errors
    Conversion(ConversionError),    // Key format conversion errors
    Password(io::Error),            // Password reading errors
    KeyOutput(io::Error),          // File writing errors
    EncryptionError(EncryptionError), // Key encryption errors
    Json(serde_json::Error),       // JSON serialization errors
    Exists(String),                // File already exists error
}
```

## Dependencies Specification

### Direct Dependencies

| Crate | Version | Purpose |
|-------|---------|---------|
| `base64` | workspace | Base64 encoding/decoding |
| `clap` | workspace | Command-line argument parsing |
| `openssl` | workspace | RSA cryptographic operations |
| `operator_key` | workspace | SSV-specific key formatting |
| `rpassword` | 7.4.0 | Secure password input |
| `serde` | workspace | Serialization support |
| `serde_json` | workspace | JSON serialization |
| `thiserror` | workspace | Error handling macros |
| `tracing` | workspace | Structured logging |
| `zeroize` | workspace | Secure memory management |

### Key Dependency Details

#### `operator_key` Integration
- **Modules Used**:
  - `encrypted::EncryptedKey` - EIP-2335 compliant key encryption
  - `public::to_base64()` - SSV protocol public key format
  - `unencrypted::to_base64()` - Base64 PKCS1 private key format
  - `ConversionError` - Unified conversion error handling

#### `openssl` Usage
- **Key Generation**: `Rsa::generate(2048)` for RSA key pair creation
- **Types**: `Rsa<Private>` for private key representation
- **Error Handling**: `ErrorStack` for cryptographic operation errors

## File System Specifications

### Output Files

#### Unencrypted Mode
- **Private Key**: `private_key.txt`
  - Format: Base64-encoded PKCS1 private key
  - Compatibility: go-ssv format
  - Security: Unencrypted plaintext

#### Encrypted Mode
- **Private Key**: `encrypted_private_key.json`
  - Format: EIP-2335 compliant JSON keystore
  - Encryption: AES-128-CTR with PBKDF2 key derivation
  - Additional Fields: Includes `pubKey` field for verification

#### Public Key (Both Modes)
- **File**: `public_key.txt`
- **Format**: Base64-encoded SSV protocol compatible format
- **Usage**: For on-chain operator registration

### Directory Structure
```
<data_dir>/
├── private_key.txt (unencrypted mode)
├── encrypted_private_key.json (encrypted mode)
└── public_key.txt (always present)
```

## Security Specifications

### Cryptographic Standards
- **Key Algorithm**: RSA
- **Key Size**: 2048 bits
- **Random Number Generation**: OpenSSL secure random
- **Encryption Standard**: EIP-2335 (when encryption enabled)
- **Encryption Algorithm**: AES-128-CTR
- **Key Derivation**: PBKDF2

### Memory Security
- **Password Handling**: `Zeroizing<String>` containers
- **Automatic Cleanup**: Memory zeroed on drop
- **Secure Input**: Masked password prompts via `rpassword`

### File Security
- **Overwrite Protection**: Prevents accidental key replacement
- **Force Override**: Explicit `--force` flag required for overwrite
- **Atomic Operations**: Keys written completely or not at all

## CLI Interface Specification

### Command Structure
```bash
keygen [OPTIONS]
```

### Options
- `--force` - Force file overwrite (default: false)
- `--encrypt` - Enable password encryption
- `--password-file <PATH>` - Path to password file (requires --encrypt)

### Exit Codes
- `0` - Success
- `1` - General error (see error message for details)

## Performance Specifications

### Time Complexity
- **Key Generation**: O(1) - constant time RSA generation
- **Key Conversion**: O(1) - linear in key size (constant for 2048-bit)
- **File I/O**: O(1) - single write operations per file

### Memory Usage
- **Peak Memory**: ~4KB for 2048-bit RSA key
- **Password Storage**: Minimal, immediately zeroized
- **Working Set**: <1MB total memory footprint

### Disk I/O
- **Write Operations**: 2-3 files (depending on encryption mode)
- **File Sizes**: 
  - Private key: ~1.6KB (unencrypted) or ~600 bytes (encrypted JSON)
  - Public key: ~372 bytes

## Error Handling Specification

### Error Categories
1. **Cryptographic Errors**: OpenSSL failures, key generation issues
2. **I/O Errors**: File system access, permission issues
3. **User Input Errors**: Password reading, validation failures
4. **Configuration Errors**: Invalid options, missing requirements
5. **Format Errors**: Key conversion, JSON serialization issues

### Error Recovery
- **Transient Errors**: Retry prompts for password mismatches
- **Fatal Errors**: Clean exit with descriptive error messages
- **Partial Failures**: No partial key file creation on errors

## Integration Specifications

### Anchor Client Integration
- **Key Loading**: Generated keys read by main Anchor client
- **Format Compatibility**: Direct compatibility with client key loading
- **Path Conventions**: Standard data directory structure

### SSV Network Integration
- **Public Key Format**: Compatible with SSV operator registration
- **Protocol Compliance**: Follows SSV key format specifications
- **Network Interoperability**: Works with existing SSV tooling