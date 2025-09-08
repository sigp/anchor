# Anchor RSA Key Generation Tool
A secure RSA key generation tool for SSV Operator nodes. The generated public key is used to register the operator on-chain, while the private key is used by the Anchor node. 

# Usage 
## Basic Key Generation
```bash
anchor keygen
```
This creates: 
- `unencrypted_private_key.txt` - Contains the **unencrypted** private key
- `public_key.txt` - Contains BASE-64 format public key

## With password protection
```bash
anchor keygen --encrypt
```
You will be prompted for a password, unless you specify a password file via `--password-file`.

This creates:
- `encrypted_private_key.json` - Contains the encrypted private key.
- `public_key.txt` - Contains BASE-64 format public key.

Make sure to provide the password via `--password-file` when running the Anchor node, or input it at startup. 

## Deterministic Key Generation

Generate deterministic RSA keys from BIP39 mnemonic seeds. This allows you to recreate the same keys deterministically from a mnemonic phrase.

### Generate with new mnemonic
```bash
anchor keygen --deterministic
```
This will generate a new 24-word BIP39 mnemonic and derive an RSA key from it. **Save the mnemonic phrase securely** as you'll need it to regenerate the same key.

### Generate from existing mnemonic
```bash
anchor keygen --deterministic --mnemonic "your twelve or twenty four word mnemonic phrase here"
```

### Generate from mnemonic file
```bash
anchor keygen --deterministic --mnemonic-file /path/to/mnemonic.txt
```

### Multiple keys from same mnemonic
You can generate different keys from the same mnemonic using the derivation index:
```bash
anchor keygen --deterministic --mnemonic "your mnemonic..." --index 0  # First key
anchor keygen --deterministic --mnemonic "your mnemonic..." --index 1  # Second key
```

### Deterministic + Encryption
Combine deterministic generation with password protection:
```bash
anchor keygen --deterministic --encrypt --mnemonic "your mnemonic..."
```

## Custom Output Directory
```bash
anchor keygen --data-dir path/to/directory
```

## Force Overwrite Existing Key Files
```bash
anchor keygen --force
```

# Deterministic Key Generation Details

The deterministic key generation uses:
- **BIP39** mnemonic phrases (12 or 24 words)
- **HKDF-SHA256** for key material derivation
- **Cryptographically secure prime generation** with deterministic starting points

This method ensures:
- Same mnemonic + index always produces the same RSA key
- Different indices produce different keys from the same mnemonic
- Keys are cryptographically secure and suitable for production use
- Full compatibility with BIP39 standards

**Security Note**: Keep your mnemonic phrase secure and backed up. Anyone with the mnemonic can regenerate your keys.


