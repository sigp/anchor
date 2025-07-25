# Anchor Logging Component AI Documentation

## Overview
The logging component provides comprehensive logging infrastructure for the Anchor validator client, including file-based logging, metrics collection, and specialized tracing layers for libp2p and discv5 network components.

## Key Components

### 1. File Logging (`logging.rs`)
- **Purpose**: Configurable file-based logging with rotation and compression
- **Key Structures**:
  - `FileLoggingFlags`: CLI configuration for log file settings
  - `LoggingLayer`: Wrapper for non-blocking writer and worker guard
- **Key Functions**:
  - `init_file_logging()`: Initializes rolling file appender with size-based rotation

### 2. Metrics Collection (`count_layer.rs`)
- **Purpose**: Tracks logging event counts for monitoring and observability
- **Metrics Tracked**:
  - Global counters: `INFOS_TOTAL`, `WARNS_TOTAL`, `ERRORS_TOTAL`
  - Per-dependency counters: `DEP_INFOS_TOTAL`, `DEP_WARNS_TOTAL`, `DEP_ERRORS_TOTAL`
- **Implementation**: `CountLayer` implements tracing subscriber layer

### 3. Network Tracing (`tracing_libp2p_discv5_layer.rs`)
- **Purpose**: Specialized logging for libp2p gossipsub and discv5 network protocols
- **Features**:
  - Separate log files for libp2p and discv5 components
  - Custom timestamp formatting
  - Size-based log rotation

### 4. Workspace Filtering (`utils.rs`)
- **Purpose**: Filters logs to only include workspace crate messages
- **Function**: `build_workspace_filter()` creates filter for workspace-only logging

## Architecture

```
logging/
├── lib.rs           - Module exports and public API
├── logging.rs       - File logging configuration and initialization
├── count_layer.rs   - Metrics collection layer
├── tracing_libp2p_discv5_layer.rs - Network protocol tracing
└── utils.rs         - Workspace filtering utilities
```

## Dependencies
- `tracing` and `tracing-subscriber`: Core tracing infrastructure
- `logroller`: Rolling file appender with compression
- `metrics`: Prometheus-style metrics collection
- `chrono`: Timestamp formatting
- `clap`: CLI argument parsing

## Integration Points
- Integrates with Anchor's metrics system for monitoring
- Uses workspace member detection for filtering
- Provides CLI flags through `FileLoggingFlags`
- Exports public API through `lib.rs`

## Design Patterns
- **Builder Pattern**: Used in `LogRollerBuilder` for log file configuration
- **Layer Pattern**: Implements tracing subscriber layers for modular functionality
- **Static Lazy Initialization**: Metrics counters use `LazyLock` for thread-safe initialization
- **Non-blocking I/O**: Uses non-blocking writers to prevent log I/O from blocking application threads