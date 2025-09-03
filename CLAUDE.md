# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## About Anchor

Anchor is an open-source implementation of the Secret Shared Validator (SSV) protocol, written in Rust and maintained by Sigma Prime. It serves as a validator client for Ethereum's proof-of-stake consensus mechanism using secret sharing techniques.

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

Anchor is a multi-threaded client with several core components organized as a modular Rust workspace. The architecture follows a service-oriented approach with well-defined boundaries between components.

### Core Design Principles

1. **Modularity**: Components are separated into their own crates with clear boundaries
2. **Error Handling**: Comprehensive error types specific to each module
3. **Asynchronous Design**: Built on Tokio for non-blocking operations
4. **Thread Safety**: Uses Arc, Mutex, RwLock appropriately for shared state
5. **Message Passing**: Communication between components via channels

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

### Key Components In Detail

#### Consensus (QBFT)

The QBFT module implements the Quorum Byzantine Fault Tolerance consensus algorithm:
- Located in `anchor/common/qbft`
- State machine-based implementation
- Supports pluggable network and validation layers
- Thread-safe for concurrent operation
- Includes comprehensive testing for consensus edge cases

#### Signature Collection

The Signature Collector manages distributed validator signatures:
- Located in `anchor/signature_collector`
- Collects partial signatures from distributed validator operators
- Uses threshold signature schemes with Lagrange interpolation
- Handles timeouts and failure modes
- Reconstructs full signatures when threshold is reached

#### Network Layer

The network component handles P2P communication:
- Based on libp2p
- Supports encrypted communications
- Handles peer discovery and connection management
- Routes messages to appropriate internal components

### General Event Flow

1. The Duties Service identifies a validator duty
2. The duty is sent to the Processor
3. The Processor creates a QBFT instance
4. The Network receives messages until the QBFT instance completes
5. The required consensus message is signed
6. The message is published on the P2P network

## Code Organization

The codebase is organized as a Rust workspace with multiple crates, each with a specific responsibility:

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

## Modular Project Structure and Boundaries

Anchor follows a modular design with clear boundaries between components, emphasizing the following principles:

### Crate Structure

1. **Independent Crates**: Each major component is its own crate with a clearly defined API
2. **Minimal Dependencies**: Crates should only depend on what they need
3. **Public API Surface**: APIs between crates should be well-documented and minimal
4. **Clear Ownership**: Each crate has a clear responsibility and ownership model

### Dependency Flow

- **Common Libraries**: Core types and utilities are in `common/` subdirectories
- **Service Dependencies**: Higher-level services depend on lower-level ones, not vice versa
- **Configuration Flow**: Config flows down from the client to individual components
- **Event Flow**: Events flow up from components to central coordinators

### Inter-Component Communication

1. **Message Passing**: Components communicate via typed message channels
2. **Event Bus**: System-wide events use the EventBus pattern
3. **Trait Boundaries**: Components interact through trait interfaces, not concrete implementations
4. **Error Propagation**: Errors are properly typed and propagated up the stack

## Code Style and Best Practices

When contributing to Anchor, follow these Rust best practices:

### General Principles

1. **Follow Rust Idioms**: Use idiomatic Rust patterns (e.g., `Option`, `Result`, iterators)
2. **Error Handling**: Use proper error types and the `?` operator; avoid `unwrap()`/`expect()` in production code
3. **Memory Safety**: Leverage Rust's ownership system; avoid unsafe code when possible
4. **Documentation**: All public APIs should be documented with examples
5. **Type Safety**: Use the type system to prevent errors; avoid stringly-typed interfaces
6. **Simplicity First**: Always choose the simplest solution that elegantly solves the problem, follows existing patterns, maintains performance, and uses basic constructs over complex data structures
7. **Check Requirements First**: Before implementing or creating anything (PRs, commits, code), always read and follow existing templates, guidelines, and requirements in the codebase

### Specific Guidelines

1. **Naming**:
    - Use clear, descriptive names
    - Follow Rust naming conventions (snake_case for functions/variables, CamelCase for types)
    - Prefer explicit names over abbreviations

2. **Code Organization**:
    - Organize code into logical modules
    - Keep functions small and focused
    - Use the module system to control visibility

3. **Error Types**:
    - Create domain-specific error types using `thiserror`
    - Include context in errors
    - Make error messages user-friendly

4. **Comments**:
    - Comment "why", not "what"
    - Use doc comments (`///`) for public API documentation
    - Add `TODO`, `FIXME`, or `NOTE` markers as needed for future work

5. **Async Code**:
    - Use `async`/`.await` properly with Tokio
    - Handle cancellation correctly
    - Avoid blocking the runtime with CPU-intensive work

6. **Dependencies**:
    - Keep dependencies minimal and up to date
    - Prefer well-maintained crates from the ecosystem
    - Pin dependency versions appropriately

## Testing

**ALWAYS use the tester-subagent when creating tests.** It has expert knowledge of:
- Anchor codebase architecture and testing patterns
- QBFT consensus testing and message construction
- Bug reproduction methodology (tests that fail when bugs exist)
- API learning strategies and compilation debugging
- All crate-specific testing requirements

The tester agent includes detailed knowledge of testing best practices, common pitfalls, and Anchor-specific patterns for creating reliable tests.

## Specialized Agents

Use these agents proactively for their specific domains:
- **tester-subagent**: Use immediately when creating any tests
- **code-reviewer-subagent**: Use immediately after writing or modifying Rust code
- **qbft-subagent**: Use for any QBFT specification compliance questions

## Contribution Workflow

When contributing to Anchor, follow these steps to ensure high-quality code that meets project standards:

### Pull Request Requirements

**PR Title:** Must follow [Conventional Commits](https://www.conventionalcommits.org/en/v1.0.0/) format as enforced by `.github/workflows/pr-checks.yml`:
- `feat: add new user login`
- `fix: correct button size`
- `docs: update README`
- `test: add QBFT consensus tests`
- `chore: update dependencies`
- `perf: optimize message processing`
- `refactor: simplify validation logic`
- `ci: update workflow configuration`
- `revert: undo previous change`

**Breaking Changes:** Use `!` for breaking changes (e.g., `feat!: changed the API`)

**PR Description:** **ALWAYS read `.github/PULL_REQUEST_TEMPLATE.md` first**, then follow the template format exactly:
- **Issue Addressed:** Which issue # does this PR address?
- **Proposed Changes:** List or describe the changes introduced
- **Additional Info:** Future considerations or information for reviewers

### Step 1: Plan Your Changes

- Start with a clear understanding of the problem or feature
- Break down complex tasks into smaller, manageable steps
- Consider how your change affects the overall architecture
- Discuss significant changes with the team before implementing

### Step 2: Development Process

1. **Branch**: Create a feature branch from the `unstable` branch
2. **Implement**: Write code following project style guidelines
3. **Test**: Add tests that cover your changes
4. **Document**: Update documentation as needed
5. **Refactor**: Clean up code before submission

### Step 3: Pre-Commit Quality Checks

**MANDATORY:** Before committing any code changes, run:

```bash
# Format code (required)
make cargo-fmt
# or
cargo +nightly fmt --all

# Check formatting
make cargo-fmt-check

# Run linter (required) 
make lint
# or
cargo clippy --workspace --tests --features "$(TEST_FEATURES)" -- -D warnings

# Run tests
make test
```

**Additional Quality Checks:**
- **Check Performance**: Consider performance implications
- **Ensure Backwards Compatibility**: When applicable
- **Run Audit**: `make audit` for security issues (when dependencies change)

### Step 4: Submit Changes

1. **Commit**: Use clear commit messages that explain the change
2. **Push**: Push your branch to your fork
3. **PR**: Open a pull request against the `unstable` branch
4. **Review**: Address review feedback promptly
5. **CI**: Ensure all CI checks pass

### Commit Message Guidelines

- Use present tense ("Add feature", not "Added feature")
- First line is a summary (50 chars or less)
- Include component prefix (e.g., `network:`, `consensus:`)
- Reference issues or tickets when applicable
- Include context on why the change was made

### PR Description Best Practices

When writing PR descriptions, follow these guidelines for maintainable and reviewable documentation:

- **Keep "Proposed Changes" section high-level** - focus on what components were changed and why
- **Avoid line-by-line documentation** - reviewers can see specific changes in the diff
- **Use component-level summaries** rather than file-by-file breakdowns  
- **Emphasize the principles** being applied and operational impact
- **Be concise but complete** - provide context without overwhelming detail

## Development Tips

- This is a Rust project that follows standard Rust development practices
- The project is currently under active development and not ready for production
- Sigma Prime maintains two permanent branches:
    - `stable`: Always points to the latest stable release, ideal for most users
    - `unstable`: Used for development, contains the latest PRs, base branch for contributions
- When implementing new features, focus on modular design with clear boundaries
- Follow test-driven development principles when possible
- Use debugging tools like `tracing` and metrics to understand system behavior

## Session Learning Updates

After successful Claude Code sessions where the user is satisfied with results, update both CLAUDE.md and relevant specialized agents with general principles learned:

- **CLAUDE.md**: Add universal principles that apply across all development contexts
- **Specialized Agents**: Update each agent with context-specific lessons learned in their domain
- **Focus on Principles**: Capture the underlying reasoning and approach, not implementation details
- **Generalize Lessons**: Extract principles that can be applied to similar future problems