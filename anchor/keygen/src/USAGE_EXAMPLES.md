# Keygen Component - Usage Examples

## Basic Usage Examples

### Example 1: Simple Key Generation

```rust
use keygen::{run_keygen, Keygen};
use std::path::Path;

// Create basic keygen configuration
let keygen_config = Keygen {
    force: false,
    encrypt: false,
    password_file: None,
};

// Generate keys in the current directory
let data_dir = Path::new("./keys");
match run_keygen(keygen_config, data_dir) {
    Ok(private_key) => {
        println!("Successfully generated RSA key pair");
        // private_key is the OpenSSL Rsa<Private> object
    },
    Err(e) => {
        eprintln!("Key generation failed: {}", e);
    }
}
```

**Output Files**:
- `./keys/private_key.txt` - Unencrypted private key in base64 format
- `./keys/public_key.txt` - Public key for SSV registration

### Example 2: Encrypted Key Generation

```rust
use keygen::{run_keygen, Keygen};
use std::path::{Path, PathBuf};

// Create configuration for encrypted key generation
let keygen_config = Keygen {
    force: false,
    encrypt: true,
    password_file: None, // Will prompt user for password
};

let data_dir = Path::new("./secure_keys");
match run_keygen(keygen_config, data_dir) {
    Ok(private_key) => {
        println!("Successfully generated encrypted RSA key pair");
    },
    Err(e) => {
        eprintln!("Encrypted key generation failed: {}", e);
    }
}
```

**Interactive Prompts**:
```
Enter password for keyfile: [hidden input]
Re-enter password to confirm: [hidden input]
```

**Output Files**:
- `./secure_keys/encrypted_private_key.json` - EIP-2335 encrypted private key
- `./secure_keys/public_key.txt` - Public key for SSV registration

### Example 3: Using Password File

```rust
use keygen::{run_keygen, Keygen};
use std::path::{Path, PathBuf};

// Create configuration with password file
let keygen_config = Keygen {
    force: false,
    encrypt: true,
    password_file: Some(PathBuf::from("./password.txt")),
};

let data_dir = Path::new("./automated_keys");
match run_keygen(keygen_config, data_dir) {
    Ok(private_key) => {
        println!("Successfully generated keys using password file");
    },
    Err(e) => {
        eprintln!("Key generation with password file failed: {}", e);
    }
}
```

**Required Files**:
- `./password.txt` containing the encryption password

### Example 4: Force Overwrite Existing Keys

```rust
use keygen::{run_keygen, Keygen};
use std::path::Path;

// Configuration to overwrite existing keys
let keygen_config = Keygen {
    force: true,  // This will overwrite existing files
    encrypt: false,
    password_file: None,
};

let data_dir = Path::new("./existing_keys");
match run_keygen(keygen_config, data_dir) {
    Ok(private_key) => {
        println!("Successfully overwrote existing keys");
    },
    Err(e) => {
        eprintln!("Force key generation failed: {}", e);
    }
}
```

## Password Reading Examples

### Example 5: Custom Password Input

```rust
use keygen::read_password_from_user;

// Read password with confirmation
match read_password_from_user(true) {
    Ok(password) => {
        println!("Password successfully read and confirmed");
        // password is a Zeroizing<String> - automatically zeroed on drop
    },
    Err(e) => {
        eprintln!("Password reading failed: {}", e);
    }
}

// Read password without confirmation (for existing key decryption)
match read_password_from_user(false) {
    Ok(password) => {
        println!("Password read for decryption");
    },
    Err(e) => {
        eprintln!("Password reading failed: {}", e);
    }
}
```

## Error Handling Examples

### Example 6: Comprehensive Error Handling

```rust
use keygen::{run_keygen, Keygen, KeygenError};
use std::path::Path;

let keygen_config = Keygen {
    force: false,
    encrypt: true,
    password_file: None,
};

let data_dir = Path::new("./keys");
match run_keygen(keygen_config, data_dir) {
    Ok(private_key) => {
        println!("Key generation successful");
    },
    Err(KeygenError::Generate(ssl_error)) => {
        eprintln!("OpenSSL key generation failed: {}", ssl_error);
    },
    Err(KeygenError::Conversion(conv_error)) => {
        eprintln!("Key format conversion failed: {}", conv_error);
    },
    Err(KeygenError::Password(io_error)) => {
        eprintln!("Password reading failed: {}", io_error);
    },
    Err(KeygenError::KeyOutput(io_error)) => {
        eprintln!("Failed to write key files: {}", io_error);
    },
    Err(KeygenError::EncryptionError(enc_error)) => {
        eprintln!("Key encryption failed: {}", enc_error);
    },
    Err(KeygenError::Json(json_error)) => {
        eprintln!("JSON serialization failed: {}", json_error);
    },
    Err(KeygenError::Exists(path)) => {
        eprintln!("Key files already exist in: {}", path);
        eprintln!("Use --force to overwrite existing keys");
    },
}
```

## CLI Integration Examples

### Example 7: CLI Argument Parsing

```rust
use clap::Parser;
use keygen::Keygen;

// Parse CLI arguments
let args = Keygen::parse();

println!("Force overwrite: {}", args.force);
println!("Encryption enabled: {}", args.encrypt);
if let Some(password_file) = &args.password_file {
    println!("Password file: {}", password_file.display());
}
```

**Command Line Usage**:
```bash
# Basic key generation
./keygen

# Generate encrypted keys
./keygen --encrypt

# Use password file for encryption
./keygen --encrypt --password-file ./my_password.txt

# Force overwrite existing keys with encryption
./keygen --force --encrypt
```

## Real-World Integration Examples

### Example 8: Anchor Client Integration

```rust
use keygen::{run_keygen, Keygen};
use std::path::Path;

// Typical usage in Anchor client setup
fn setup_operator_keys(data_dir: &Path, encrypt_keys: bool) -> Result<(), Box<dyn std::error::Error>> {
    let keygen_config = Keygen {
        force: false, // Don't overwrite existing keys
        encrypt: encrypt_keys,
        password_file: None, // Interactive password input
    };
    
    match run_keygen(keygen_config, data_dir) {
        Ok(_private_key) => {
            println!("✅ Operator keys generated successfully");
            println!("📄 Public key location: {}", data_dir.join("public_key.txt").display());
            
            if encrypt_keys {
                println!("🔐 Encrypted private key location: {}", 
                    data_dir.join("encrypted_private_key.json").display());
                println!("⚠️  Remember to provide the password when starting the Anchor node");
            } else {
                println!("🔑 Private key location: {}", 
                    data_dir.join("private_key.txt").display());
                println!("⚠️  Private key is unencrypted - ensure proper file permissions");
            }
            
            Ok(())
        },
        Err(e) => {
            eprintln!("❌ Key generation failed: {}", e);
            Err(Box::new(e))
        }
    }
}
```

### Example 9: Automated Deployment Script

```rust
use keygen::{run_keygen, Keygen};
use std::path::{Path, PathBuf};
use std::fs;

// Automated key generation for deployment
fn deploy_operator_keys(
    data_dir: &Path, 
    password_file: Option<PathBuf>,
    force_regenerate: bool
) -> Result<(), Box<dyn std::error::Error>> {
    // Ensure data directory exists
    fs::create_dir_all(data_dir)?;
    
    let keygen_config = Keygen {
        force: force_regenerate,
        encrypt: password_file.is_some(),
        password_file,
    };
    
    println!("🔧 Generating operator keys for deployment...");
    
    match run_keygen(keygen_config, data_dir) {
        Ok(_) => {
            println!("✅ Deployment keys ready");
            
            // Read and display public key for registration
            let public_key_path = data_dir.join("public_key.txt");
            let public_key = fs::read_to_string(&public_key_path)?;
            
            println!("📋 Public Key for SSV Registration:");
            println!("{}", public_key.trim());
            
            Ok(())
        },
        Err(e) => {
            eprintln!("❌ Deployment key generation failed: {}", e);
            Err(Box::new(e))
        }
    }
}
```

## File Format Examples

### Example 10: Generated File Contents

**Unencrypted Private Key** (`private_key.txt`):
```
LS0tLS1CRUdJTiBSU0EgUFJJVkFURSBLRVktLS0tLQpNSUlFb3dJQkFBS0NBUUVB...
[Base64 encoded PKCS1 private key continues]
...
LS0tLS1FTkQgUlNBIFBSSVZBVEUgS0VZLS0tLS0K
```

**Public Key** (`public_key.txt`):
```
LS0tLS1CRUdJTiBSU0EgUFVCTElDIEtFWS0tLS0tCk1JSUJJakFOQmdrcWhraUc5...
[Base64 encoded public key continues]
...
LS0tLS1FTkQgUlNBIFBVQkxJQyBLRVktLS0tLQo=
```

**Encrypted Private Key** (`encrypted_private_key.json`):
```json
{
  "version": 4,
  "uuid": "12345678-1234-5678-9abc-123456789abc",
  "path": "",
  "pubkey": "LS0tLS1CRUdJTiBSU0EgUFVCTElDIEtFWS0tLS0t...",
  "crypto": {
    "kdf": {
      "function": "pbkdf2",
      "params": {
        "dklen": 32,
        "c": 262144,
        "prf": "hmac-sha256",
        "salt": "abcdef123456..."
      },
      "message": ""
    },
    "checksum": {
      "function": "sha256",
      "params": {},
      "message": "fedcba654321..."
    },
    "cipher": {
      "function": "aes-128-ctr",
      "params": {
        "iv": "123456abcdef..."
      },
      "message": "encrypted_private_key_data..."
    }
  }
}
```

## Testing Examples

### Example 11: Unit Test Structure

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    
    #[test]
    fn test_basic_key_generation() {
        let temp_dir = TempDir::new().unwrap();
        let keygen_config = Keygen {
            force: false,
            encrypt: false,
            password_file: None,
        };
        
        let result = run_keygen(keygen_config, temp_dir.path());
        assert!(result.is_ok());
        
        // Check that files were created
        assert!(temp_dir.path().join("private_key.txt").exists());
        assert!(temp_dir.path().join("public_key.txt").exists());
    }
    
    #[test]
    fn test_existing_files_error() {
        let temp_dir = TempDir::new().unwrap();
        
        // Create existing file
        std::fs::write(temp_dir.path().join("private_key.txt"), "existing").unwrap();
        
        let keygen_config = Keygen {
            force: false,
            encrypt: false,
            password_file: None,
        };
        
        let result = run_keygen(keygen_config, temp_dir.path());
        assert!(matches!(result, Err(KeygenError::Exists(_))));
    }
}
```