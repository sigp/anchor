
# Anchor Installation Guide

This guide provides step-by-step instructions for installing **Anchor**.

---

## 1. Download the Latest Release from GitHub

1. Visit the [Anchor Releases page](https://github.com/sigp/anchor/releases).
2. Download the appropriate binary for your operating system.
3. Extract the file if necessary and move the binary to a location in your `PATH` (e.g., `/usr/local/bin/`).

### Example

```bash
wget https://github.com/sigp/anchor/releases/download/<version>/anchor-<platform>.tar.gz

# Replace <version> and <platform> with the appropriate values.

tar -xvf anchor-<platform>.tar.gz
sudo mv anchor /usr/local/bin/
```

Verify the installation:

```bash
anchor --version
```

---

## 2. Run Anchor Using Docker

1. Pull the latest Anchor Docker image:

   ```bash
   docker pull sigp/anchor:latest
   ```

2. Run the Anchor container:

   ```bash
   docker run --rm -it sigp/anchor:latest --help
   ```

## 3. Clone and Build Locally

1. Clone the Anchor repository:

   ```bash
   git clone https://github.com/sigp/anchor.git
   cd anchor
   ```

2. Build the Anchor binary:

   ```bash
   cargo build --release
   ```

   The binary will be located in `./target/release/`.

3. Move the binary to a location in your `PATH`:

   ```bash
   sudo mv target/release/anchor /usr/local/bin/
   ```

Verify the installation:

```bash
anchor --version
```
