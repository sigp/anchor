# CLAUDE.md

Guidance for Claude Code when working with this repository. Detailed standards live in `.claude/rules/`.

## About Anchor

Anchor is an open-source SSV protocol implementation in Rust, maintained by Sigma Prime. It serves as an Ethereum proof-of-stake validator client using secret sharing techniques.

## Common Commands

```bash
# Build
cargo build --release
make install

# Test
make test                    # release mode (standard)
make test-debug              # debug mode
make nextest-release         # nextest (faster)
cargo test -p <crate>        # specific crate

# Format & Lint
make cargo-fmt               # format
make cargo-fmt-check         # check format
make lint                    # clippy
make lint-fix                # auto-fix

# Quality
make check-benches           # benchmark compilation
make udeps                   # unused dependencies
make sort                    # dependency ordering
make audit                   # security audit
```

## Architecture Overview

Multi-threaded, service-oriented Rust workspace built on Tokio.

**Services:** Core Client, HTTP API, Metrics, Execution Service, Duties Service, Network (libp2p), Processor, QBFT Manager

**Event flow:** Duties Service identifies duty -> Processor creates QBFT instance -> Network exchanges messages -> consensus reached -> signed message published on P2P network

**Design principles:**
- Modularity with clear crate boundaries
- Typed error handling per module
- Async-first with Tokio
- Thread safety via Arc/Mutex/RwLock
- Inter-component message passing via channels

## Code Organization

Rust workspace under `anchor/`:
- `client/` - CLI and client interface
- `common/` - Shared types: `api_types/`, `bls_lagrange/`, `global_config/`, `operator_key/`, `qbft/`, `ssv_network_config/`, `ssv_types/`, `version/`
- `database/` - Storage operations
- `duties_tracker/` - Validator duty tracking
- `eth/` - Ethereum connectivity
- `http_api/` - HTTP API
- `http_metrics/` - Metrics endpoint
- `keygen/`, `keysplit/` - Key generation and splitting
- `logging/` - Logging infrastructure
- `message_receiver/`, `message_sender/`, `message_validator/` - Message handling
- `network/` - P2P networking
- `processor/` - Task processing
- `qbft_manager/` - QBFT instance management
- `signature_collector/` - Signature aggregation
- `subnet_service/` - Subnet operations
- `validator_store/` - Validator data storage

## Specialized Agents

Use these agents proactively:
- **tester-subagent**: MANDATORY before creating/modifying any tests
- **code-reviewer-subagent**: Use after writing/modifying Rust code
- **logging-subagent**: Use for any logging/tracing task
- **qbft-subagent**: Use for QBFT specification questions

## Development Tips

- Two permanent branches: `stable` (latest release), `unstable` (development, PR base)
- Priority order: Safety > Correctness > Simplicity > Performance > Style
- Use specialized agents immediately when their expertise applies
- Validate suggestions before implementing
- Follow the principle hierarchy: Safety > Best Practices > Existing Patterns > Consistency

## Quality Principles

- Behavioral claims require evidence; label unverified claims as hypotheses
- Fix bad practices in code being modified; use best practices for all new code
- Create issues for tech debt found elsewhere; don't fix unrelated code in current PR
- Prioritize: safety fixes > performance > style improvements
