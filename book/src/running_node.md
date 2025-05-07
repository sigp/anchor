# Running an Anchor SSV Operator Node

## What is an SSV Operator

An SSV operator is a node that holds shares of validators' keys and participates in committees to perform Ethereum validation duties. The SSV network enables distributed validation where multiple operators collectively validate without any single operator having access to the complete validator key.

**Step 1: Generate RSA keys**

Anchor includes a key generation tool to create the RSA keys needed for operator identity:

```bash
# Generate unencrypted keys (for development)
anchor keygen

# Generate encrypted keys (recommended for production)
anchor keygen --password "your-secure-password" --output-path /path/to/keys/directory
```

This will generate:

- A `key.pem` file containing your private key
- The public key output in the console (for un-encrypted keys, also available in `keys.json`)

Save your public key and as you'll need it for onchain registration.

**Step 2: Register as an Operator on the SSV Network**

To register an operator, follow the instructions for the official
[ssv docs](https://docs.ssv.network/operators/operator-management/registration)

**Step 3: Configure and run your Anchor node**

Create a directory for anchor related data and move the generated `key.pem` into the directory

```bash
mkdir -p ~/.anchor

mv key.pem ~/.anchor
```

Reference the cli or use `--help` to launch the node

```bash
anchor node \
  --network mainnet \
  --datadir ~/.anchor \
  --beacon-nodes http://localhost:5052 \
  --execution-rpc http://localhost:8545 \
  --execution-ws ws://localhost:8546 \
  --metrics \
  --rsa-key-password "your-password"
```
