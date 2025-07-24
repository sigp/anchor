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

## Custom Output Directory
```bash
anchor keygen --data-dir path/to/directory
```

## Force Overwrite Existing Key Files
```bash
anchor keygen --force
```


