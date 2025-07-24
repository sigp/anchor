# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## About Anchor

Anchor is an open source implementation of the Secret Shared Validator (SSV) protocol, written in Rust and maintained by Sigma Prime. It serves as a validator client for Ethereum's proof-of-stake consensus mechanism using secret sharing techniques.

## Common Commands

### Build and Install

```bash
# Build the project in release mode
cargo build --release

# Install Anchor to your path
make install

# Build for specific architectures
make build-x86_64      # Build for x86_64 Linux (requires cross)
make build-aarch64     # Build for aarch64 Linux (requires cross)

# Create release tarballs
make build-release-tarballs
```

### Testing

```bash
# Run all tests in release mode (standard)
make test
# or
cargo test --release --features "$(TEST_FEATURES)"

# Run all tests in debug mode
make test-debug
# or
cargo test --workspace --features "$(TEST_FEATURES)"

# Run tests with nextest (faster)
make nextest-release
make nextest-debug

# Test a specific crate
cd anchor/common/qbft
cargo test

# Check benchmark code (without running benchmarks)
make check-benches
```

### Linting and Formatting

```bash
# Format code
make cargo-fmt
# or 
cargo +nightly fmt --all

# Check formatting
make cargo-fmt-check

# Run linter
make lint
# or
cargo clippy --workspace --tests --features "$(TEST_FEATURES)" -- -D warnings

# Fix linting issues automatically
make lint-fix

# Check for unused dependencies
make udeps
# or
cargo +nightly udeps --tests --all-targets --release --features "$(TEST_FEATURES)"

# Check if dependencies are sorted correctly
make sort
```

### Other Useful Commands

```bash
# Run dependency audit for security issues
make audit

# Update CLI documentation in the book
make cli-local

# Check for markdown issues
make mdlint
```

## Architecture Overview

Anchor is a multi-threaded client with several core components:

### Thread Model

Anchor consists of multiple long-standing tasks that are spawned during initialization:

1. **Core Client**: The main control flow
2. **HTTP API**: Endpoint for reading data and modifying components
3. **Metrics**: Prometheus-compatible metrics endpoint
4. **Execution Service**: Syncs SSV information from execution layer nodes
5. **Duties Service**: Watches the beacon chain for validator duties for known SSV validator shares
6. **Network**: P2P network stack (libp2p) for communication on the SSV network
7. **Processor**: Middleware that handles CPU-intensive tasks and prioritizes client workload
8. **QBFT**: Manages QBFT instances to reach consensus in SSV committees

### General Event Flow

1. The Duties Service identifies a validator duty
2. The duty is sent to the Processor
3. The Processor creates a QBFT instance
4. The Network receives messages until the QBFT instance completes
5. The required consensus message is signed
6. The message is published on the P2P network

## Code Organization

The codebase is organized as a Rust workspace with multiple crates:

- `anchor/`: Main crate with several submodules:
  - `client/`: CLI and client interface
  - `common/`: Shared types and utilities
    - `api_types/`: API data structures
    - `bls_lagrange/`: BLS cryptography implementations
    - `global_config/`: Global configuration
    - `operator_key/`: Key management
    - `qbft/`: QBFT consensus implementation
    - `ssv_network_config/`: Network configuration
    - `ssv_types/`: Core SSV data types
    - `version/`: Version information
  - `database/`: Database operations and storage
  - `duties_tracker/`: Validator duty tracking
  - `eth/`: Ethereum connectivity
  - `http_api/`: HTTP API implementation
  - `http_metrics/`: Metrics API
  - `keygen/`: Key generation
  - `keysplit/`: Key splitting for SSV
  - `logging/`: Logging infrastructure
  - `message_receiver/`: Message reception
  - `message_sender/`: Message sending
  - `message_validator/`: Message validation
  - `network/`: P2P networking
  - `processor/`: Task processing
  - `qbft_manager/`: QBFT instance management
  - `signature_collector/`: Signature aggregation
  - `subnet_service/`: Subnet operations
  - `validator_store/`: Validator data storage
  
## Development Tips

- This is a Rust project that follows standard Rust development practices
- The project is currently under active development and not ready for production
- Sigma Prime maintains two permanent branches:
  - `stable`: Always points to the latest stable release, ideal for most users
  - `unstable`: Used for development, contains the latest PRs, base branch for contributions