# Keysplit Usage Examples

## Command Line Interface Examples

### Manual Mode - Basic Usage

Split a validator key among 4 operators with manually provided data:

```bash
keysplit manual \
  --keystore-path validator_keys/keystore-123.json \
  --password "my_secure_password" \
  --owner 0x742d35cc6635c0532925a3b8d0b251e6e41d3dc6 \
  --output-path output/validator_keyshares.json \
  --operators 1,2,3,4 \
  --nonce 5 \
  --public-keys "LS0tLS1CRUdJTi...,MIIBIjANBgkqhki...,LS0tLS1CRUdJTi...,MIIBIjANBgkqhki..."
```

### Manual Mode - 7 Operator Cluster

```bash
keysplit manual \
  --keystore-path /path/to/keystore.json \
  --password "keystore_password" \
  --owner 0x742d35cc6635c0532925a3b8d0b251e6e41d3dc6 \
  --output-path keyshares_output.json \
  --operators 10,11,12,13,14,15,16 \
  --nonce 0 \
  --public-keys "key1,key2,key3,key4,key5,key6,key7"
```

### Onchain Mode - Automatic Data Fetching

```bash
keysplit onchain \
  --keystore-path validator_keys/keystore-456.json \
  --password "another_password" \
  --owner 0x742d35cc6635c0532925a3b8d0b251e6e41d3dc6 \
  --output-path automated_keyshares.json \
  --operators 5,6,7,8 \
  --rpc https://mainnet.infura.io/v3/YOUR_PROJECT_ID
```

### Large Cluster Configuration

```bash
keysplit onchain \
  --keystore-path enterprise/validator.json \
  --password "enterprise_grade_password" \
  --owner 0x742d35cc6635c0532925a3b8d0b251e6e41d3dc6 \
  --output-path enterprise_shares.json \
  --operators 1,2,3,4,5,6,7,8,9,10,11,12,13 \
  --rpc wss://eth-mainnet.g.alchemy.com/v2/YOUR_API_KEY
```

## Programmatic Usage Examples

### Basic Integration

```rust
use keysplit::{run_keysplitter, Keysplit, KeygenSubcommands, Manual, SharedKeygenOptions};
use global_config::GlobalConfig;

fn split_validator_key() -> Result<(), Box<dyn std::error::Error>> {
    let shared_options = SharedKeygenOptions {
        keystore_path: "validator.json".to_string(),
        password: "secure_password".to_string(),
        owner: "0x742d35cc6635c0532925a3b8d0b251e6e41d3dc6".parse()?,
        output_path: "keyshares.json".to_string(),
        operators: "1,2,3,4".parse()?,
    };

    let manual_config = Manual {
        shared: shared_options,
        nonce: 1,
        public_keys: load_operator_public_keys()?,
    };

    let keysplit_config = Keysplit {
        subcommand: KeygenSubcommands::Manual(manual_config),
    };

    let global_config = GlobalConfig::default();
    
    run_keysplitter(keysplit_config, global_config)?;
    Ok(())
}
```

### Error Handling Example

```rust
use keysplit::{run_keysplitter, KeysplitError};

fn handle_keysplit_errors(config: Keysplit) {
    match run_keysplitter(config, GlobalConfig::default()) {
        Ok(()) => println!("Key splitting completed successfully"),
        Err(KeysplitError::Keystore(msg)) => {
            eprintln!("Keystore error: {}", msg);
            eprintln!("Check file path and permissions");
        },
        Err(KeysplitError::Password(msg)) => {
            eprintln!("Password error: {}", msg);
            eprintln!("Verify keystore password is correct");
        },
        Err(KeysplitError::InvalidOperator(msg)) => {
            eprintln!("Invalid operator: {}", msg);
            eprintln!("Ensure all operator IDs exist in the network");
        },
        Err(e) => eprintln!("Unexpected error: {:?}", e),
    }
}
```

## Output Examples

### Successful Key Split Output

```json
{
  "encrypted_keyshares": [
    {
      "id": 1,
      "public_key": {
        "n": "00c1a2b3c4d5e6f7...",
        "e": "010001"
      },
      "share_public_key": "0x8b9a7c5d2e1f0g3h...",
      "encrypted_keyshare": "a1b2c3d4e5f6g7h8..."
    },
    {
      "id": 2,
      "public_key": {
        "n": "00d1e2f3g4h5i6j7...",
        "e": "010001"
      },
      "share_public_key": "0x9c8b7a6d5e4f3g2h...",
      "encrypted_keyshare": "b2c3d4e5f6g7h8i9..."
    }
  ],
  "validator_public_key": "0x1a2b3c4d5e6f7g8h9i0j...",
  "owner": "0x742d35cc6635c0532925a3b8d0b251e6e41d3dc6",
  "nonce": 5,
  "cluster_info": {
    "operators": [1, 2, 3, 4],
    "threshold": 3
  }
}
```

## Common Use Cases

### Development Environment Setup

```bash
# Local testnet with known operator keys
keysplit manual \
  --keystore-path testnet/validator.json \
  --password "test123" \
  --owner 0x742d35cc6635c0532925a3b8d0b251e6e41d3dc6 \
  --output-path testnet_shares.json \
  --operators 1,2,3,4 \
  --nonce 0 \
  --public-keys "$(cat test_keys.txt)"
```

### Production Deployment

```bash
# Production mainnet with onchain verification
keysplit onchain \
  --keystore-path /secure/validator.json \
  --password "$KEYSTORE_PASSWORD" \
  --owner $VALIDATOR_OWNER \
  --output-path /secure/output/keyshares.json \
  --operators $SELECTED_OPERATORS \
  --rpc $MAINNET_RPC_URL
```

### Batch Processing Script

```bash
#!/bin/bash
# Process multiple validators
for keystore in validator_keys/*.json; do
    filename=$(basename "$keystore" .json)
    keysplit onchain \
        --keystore-path "$keystore" \
        --password "$KEYSTORE_PASSWORD" \
        --owner "$VALIDATOR_OWNER" \
        --output-path "output/${filename}_shares.json" \
        --operators "$OPERATOR_IDS" \
        --rpc "$RPC_ENDPOINT"
done
```

## Integration Patterns

### CI/CD Pipeline Integration

```yaml
# GitHub Actions example
- name: Split Validator Keys
  run: |
    keysplit onchain \
      --keystore-path ${{ secrets.KEYSTORE_PATH }} \
      --password ${{ secrets.KEYSTORE_PASSWORD }} \
      --owner ${{ vars.VALIDATOR_OWNER }} \
      --output-path keyshares.json \
      --operators ${{ vars.OPERATOR_IDS }} \
      --rpc ${{ secrets.RPC_ENDPOINT }}
```

### Docker Container Usage

```dockerfile
FROM rust:1.70 AS builder
COPY . .
RUN cargo build --release

FROM debian:bullseye-slim
COPY --from=builder /target/release/keysplit /usr/local/bin/
ENTRYPOINT ["keysplit"]
```

```bash
# Run in container
docker run --rm -v $(pwd)/keys:/keys keysplit:latest onchain \
  --keystore-path /keys/validator.json \
  --password "$PASSWORD" \
  --owner "$OWNER" \
  --output-path /keys/shares.json \
  --operators "1,2,3,4" \
  --rpc "$RPC_URL"
```

## Troubleshooting Examples

### Common Error Scenarios

```bash
# Invalid operator count
keysplit manual --operators 1,2,3  # Error: Must be 4, 7, 10, or 13

# Mismatched key count
keysplit manual --operators 1,2,3,4 --public-keys "key1,key2"  # Error: Count mismatch

# Invalid keystore password
keysplit manual --password "wrong"  # Error: Invalid password

# Non-existent operator (onchain mode)
keysplit onchain --operators 999  # Error: Operator does not exist
```

### Verification Commands

```bash
# Verify output file was created
ls -la keyshares.json

# Check JSON structure
jq '.' keyshares.json

# Validate operator count matches
jq '.encrypted_keyshares | length' keyshares.json

# Extract operator IDs
jq '.encrypted_keyshares[].id' keyshares.json
```