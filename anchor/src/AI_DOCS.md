# Anchor Binary AI Documentation

## Overview

The Anchor binary is the main entry point for the SSV network node implementation. It provides a complete command-line interface for running an SSV validator node, performing key generation, and handling distributed key management operations.

## Core Functionality

### Primary Commands
- **Node**: Starts the main SSV validator node with full network participation
- **Keygen**: Generates BLS keys for validator operations
- **Keysplit**: Performs distributed key splitting for multi-party computation

### Key Features

1. **Multi-threaded Runtime**: Uses Tokio async runtime with configurable threading
2. **Graceful Shutdown**: Handles SIGTERM, SIGINT, and SIGHUP signals properly
3. **Comprehensive Logging**: File-based and console logging with configurable levels
4. **Environment Management**: Cross-platform environment handling (Unix/Windows)
5. **Configuration Management**: CLI-based configuration with global flags

## Architecture

The binary follows a modular architecture pattern:

```
Anchor Binary
├── CLI Parsing (clap)
├── Global Configuration
├── Environment Setup
├── Logging Infrastructure
└── Command Dispatch
    ├── Node (Full SSV node)
    ├── Keygen (Key generation)
    └── Keysplit (Key splitting)
```

## Dependencies

### Core Dependencies
- **client**: Main SSV client implementation
- **global_config**: Global configuration management
- **logging**: Structured logging with tracing
- **task_executor**: Async task management
- **keygen**: BLS key generation utilities
- **keysplit**: Distributed key splitting

### External Dependencies
- **tokio**: Async runtime
- **clap**: Command-line argument parsing
- **tracing**: Structured logging
- **futures**: Async utilities

## Configuration

The binary accepts global flags that configure:
- Data directory location
- Debug levels
- Network parameters
- Logging options

## Error Handling

Robust error handling with:
- Graceful degradation on configuration failures
- Proper error propagation to CLI
- Shutdown on critical failures
- Comprehensive error logging

## Platform Support

- **Unix**: Full signal handling support
- **Windows**: Platform-specific signal handling via `environment_windows.rs`

## Integration Points

The binary integrates with:
- SSV network protocol
- Ethereum beacon chain
- Lighthouse validator infrastructure
- BLS signature schemes
- SQLite databases for persistence