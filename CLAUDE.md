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

### Architecture and Design

1. **Question Intermediaries**: If data flows A → B with no transformation, question why A → intermediate → B exists. Each layer should provide clear value (logging, transformation, validation, etc.). Ask: "What problem does this solve that direct communication doesn't?"

2. **Separation Through Interfaces, Not Layers**: Clean boundaries come from well-defined APIs, not intermediary components. A component receiving a `Sender<Event>` achieves separation without needing forwarding tasks or wrapper channels.

3. **Simplification is Always Valid**: Refactoring working code for simplicity is encouraged. Question architectural decisions even after tests pass. Fewer lines and fewer components often indicates better design.

4. **Challenge Complexity**: Every abstraction should justify its existence. "We might need it later" or "it provides separation" aren't sufficient reasons. Complexity must solve specific, current problems.

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

### Database Testing Patterns

Anchor uses two types of test fixtures for database testing:

1. **InMemoryTestFixture**: Fast tests using SQLite in-memory databases (`:memory:`)
   - Use for unit tests and fast integration tests
   - No file I/O overhead
   - Data is lost when connection closes

2. **FileTestFixture**: Tests requiring persistence or restart simulation
   - Use for testing database migrations, restarts, or cross-process scenarios
   - Uses temporary files that are automatically cleaned up
   - Data persists until TempDir is dropped

## Universal Code Quality Principles

All agents and contributors must follow these fundamental principles:

### Production Safety Requirements 
- **Never use `.unwrap()` or `.expect()` without clear safety justification** - always use proper Result/Option handling
- **Validate all user inputs** and handle potential failure cases gracefully
- **No secrets or sensitive data** in logs, error messages, or debug output
- **Memory safety first** - leverage Rust's ownership system, avoid unsafe code without justification

### API and Dependency Management

**Critical Principle**: Never suggest functionality that doesn't exist in dependencies.

- **Check `Cargo.toml` for exact versions** before suggesting any dependency APIs
- **Verify methods exist** in the specific versions used - never assume latest documentation applies
- **Read dependency public APIs carefully** before recommending features or methods
- **Search existing codebase** for established patterns, but prioritize best practices over bad existing patterns
- **Don't assume capabilities** - external dependencies may have architectural constraints that prevent certain patterns
- **Check implementation details** - if you don't see a method in the public API, don't suggest creating or using it
- **When uncertain, verify** - check the dependency's source code or ask the user before suggesting
- **Don't extrapolate** - don't assume dependencies support common patterns if they have different design requirements

### Incremental Improvement Strategy
- **Fix bad practices in code being modified** - if you're touching it, improve it
- **Use best practices for all new code** - never add to technical debt
- **Create GitHub issues for technical debt found elsewhere** - don't fix unrelated code in current PR
- **Prioritize safety fixes** over performance optimizations over style improvements

### Agent Usage Requirements  
- **Use specialized agents immediately** when their expertise applies - don't wait for users to ask
- **Follow the principle hierarchy**: Safety → Best Practices → Existing Patterns → Consistency
- **Validate suggestions** before implementing - agents should verify their recommendations work

## Specialized Agents

Use these agents proactively to prevent errors and enforce quality standards:

- **tester-subagent**: **Use immediately when creating any tests.** Expert in Anchor architecture, QBFT consensus testing, and bug reproduction methodology.

- **code-reviewer-subagent**: **Use immediately after writing or modifying Rust code.** Reviews for safety, memory management, idiomatic patterns, and performance.

- **logging-subagent**: **Use for any logging/tracing task.** Improves structured logging, reduces noise, and enforces performance patterns in Anchor's tracing infrastructure.

- **qbft-subagent**: **Use for QBFT specification questions.** Expert on EEA QBFT v1 Dafny L1 specification, predicates, events, and invariants.

Proactive agent usage prevents compilation errors from incorrect APIs, catches safety issues like `.unwrap()` before production, and saves debugging time by catching problems early.

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
- **Don't mention implementation details** - avoid specifying exact files, line numbers, or function names
- **Don't state the obvious** - don't mention that tests pass (CI will verify this)
- **Avoid redundancy** - don't repeat information already in the title or commit message
- **Focus on the "why"** - explain the motivation and impact, not the mechanics

### Code Review Culture

Effective code reviews question "why" architectural decisions exist:

**Questions to Ask:**
- "Why does this intermediary layer exist?"
- "What problem does this abstraction solve?"
- "Could components communicate directly?"
- "Is this complexity providing clear value?"

**Encourage Simplification:**
- Working code can still be improved
- Refactoring for clarity is valuable
- Fewer components usually means better architecture
- Test passing ≠ design complete

**Balance:**
- Question complexity, but respect existing patterns that solve real problems
- Not every layer is unnecessary - some provide genuine value
- Focus on "why" over "what"

## Development Tips

- This is a Rust project that follows standard Rust development practices
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