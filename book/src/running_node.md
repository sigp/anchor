# Running an Anchor SSV Operator Node

## What is an SSV Operator

An SSV operator is a node that holds shares of validators' keys and participates in committees to perform Ethereum validation duties. The SSV network enables distributed validation where multiple operators collectively validate without any single operator having access to the complete validator key.

If you want to migrate an existing key from the Golang implementation of SSV, you can directly proceed to Step 3.

**Step 1: Generate RSA keys**

Anchor includes a key generation tool to create the RSA keys needed for operator identity:

```bash
# Generate unencrypted keys (for development)
anchor keygen

# Generate encrypted keys (recommended for production)
anchor keygen --encrypt --output-path /path/to/keys/directory
```

This will generate:

- Your private key. If you choose to not encrypt your key, the file will be called `private_key.txt`. For encrypted keys, the file will be called `encrypted_private_key.json`.
- The public key output in the console and a file called `public_key.txt`.

Save your public key as you'll need it for on-chain registration. **Back up your key as it cannot be restored if lost!**

**Step 2: Register as an Operator on the SSV Network**

To register an operator, follow the instructions for the official
[ssv docs](https://docs.ssv.network/operators/operator-management/registration).

**Step 3: Configure and run your Anchor node**

Create a directory for Anchor-related data and move the generated private key into the directory. By default, Anchor
uses `~/.anchor/<network>`, where `<network>` is `hoodi` or `holesky`. We use `hoodi` below:

```bash
mkdir -p ~/.anchor/hoodi

mv encrypted_private_key.json ~/.anchor/hoodi
```

Use the [CLI Reference](./cli.md) or `--help` to launch the node. If you use an encrypted key, you must specify the password via a password file or interactively input it when starting the node.

```bash
anchor node \
  --network hoodi \
  --datadir ~/.anchor/hoodi \
  --beacon-nodes http://localhost:5052 \
  --execution-rpc http://localhost:8545 \
  --execution-ws ws://localhost:8546 \
  --password-file /path/to/file
```

All options used in this example (except for the `password-file`) are actually used with the default values and can therefore be omitted, or adjusted to your setup.

The Anchor node will use the same ports as used by Go-SSV unless explicitly overridden. See [Advanced Networking](./advanced_networking.md) for more information
