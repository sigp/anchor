# Anchor RSA Key Generation Tool
A secure RSA key generation tool for SSV Operator nodes. The generated public key is used to register the operator on-chain, while the private key is used by the Anchor node. 

# Usage 
## Basic Key Generation
```bash
anchor keygen
```
This creates: 
- `key.pem` - Contains the **unencrypted** private key
- `keys.json` - Contains BASE-64 format public and private keys

## With password protection
```bash
anchor keygen --password "your-secure-password"
```
Generates an encrypted `key.pem` file and outputs the public key to the console. Make sure to provide the password via `--rsa-key-password` when running the Anchor node. 

## Custom Output Directory
```bash
anchor keygen --output-path path/to/directory
```

## Force Overwrite Existing Key Files
```bash
anchor keygen --force
```


