# Anchor Binary Component Specification

## Technical Specifications

### Binary Details
- **Name**: anchor
- **Version**: 0.2.0
- **Rust Edition**: 2024
- **Minimum Rust Version**: 1.88.0
- **Default Run Target**: anchor

### Command Line Interface

#### Global Flags (`GlobalFlags`)
Common flags available across all subcommands through the global configuration system.

#### Subcommands Structure
```rust
pub enum AnchorSubcommands {
    Node(Box<Node>),     // Full SSV node operation
    Keysplit(Keysplit),  // Distributed key splitting
    Keygen(Keygen),      // BLS key generation
}
```

### Environment Management

#### Environment Struct
```rust
pub struct Environment {
    runtime: Arc<Runtime>,                    // Tokio runtime
    signal_rx: Option<Receiver<ShutdownReason>>, // Shutdown receiver
    signal_tx: Sender<ShutdownReason>,        // Shutdown sender
    signal: Option<async_channel::Sender<()>>, // Exit signal
    exit: async_channel::Receiver<()>,        // Exit receiver
}
```

#### Runtime Configuration
- **Type**: Multi-threaded Tokio runtime
- **Features**: All features enabled (`enable_all()`)
- **Shutdown Timeout**: 15 seconds maximum
- **Task Executor**: Weak reference architecture for graceful shutdown

### Signal Handling

#### Unix Platforms
- **SIGTERM**: Graceful termination
- **SIGINT**: Interrupt (Ctrl+C)
- **SIGHUP**: Hangup signal
- **Implementation**: Async signal handling with `tokio::signal::unix`

#### Windows Platform
- **Implementation**: Platform-specific handling via `environment_windows.rs`
- **Signals**: Windows-equivalent signals

### Logging Architecture

#### Logging Layers
1. **Console Layer**: Formatted output with environment-based filtering
2. **File Layer**: Configurable file-based logging with rotation
3. **libp2p/discv5 Layer**: Network-specific logging
4. **Count Layer**: Metrics and counting functionality

#### Configuration Options
- **Debug Levels**: Configurable per-component
- **File Logging**: Optional with size limits and color options
- **Workspace Filtering**: Module-specific log filtering
- **Environment Variables**: `RUST_LOG` support

### Compilation Features

#### Optional Features
- **`portable`**: Portable BLS crypto compilation (`bls/supranational-portable`)
- **`modern`**: Force ADX instructions (`bls/supranational-force-adx`)
- **`spec-minimal`**: Minimal specification support (testing only)

### Error Handling Strategy

#### Error Propagation
- Configuration errors: Early exit with error message
- Runtime errors: Logged and propagated to shutdown system
- Critical failures: Immediate termination
- Task failures: Graceful degradation where possible

#### Shutdown Reasons
```rust
enum ShutdownReason {
    Success(&'static str),
    Failure(String),
}
```

### Memory Management

#### Arc Usage
- Runtime shared via `Arc<Runtime>`
- Task executor uses weak references
- Graceful cleanup on shutdown

#### Channel Management
- Bounded channels for shutdown signaling
- Unbounded channels for exit coordination
- Proper channel cleanup on termination

### Integration Specifications

#### Ethereum Integration
- **Spec Support**: Mainnet and Minimal (conditional)
- **Network**: Configurable via `ssv_network_config`
- **Domain Types**: SSV domain type configuration

#### Task Management
- **Executor**: Custom task executor with shutdown coordination
- **Spawning**: Named task spawning for debugging
- **Lifecycle**: Complete task lifecycle management

### Performance Characteristics

#### Startup Time
- Fast startup with minimal initialization overhead
- Lazy loading of heavy components
- Early error detection and reporting

#### Resource Usage
- Multi-threaded async execution
- Memory-efficient Arc/Weak reference patterns
- Configurable logging overhead

#### Shutdown Performance
- Maximum 15-second shutdown timeout
- Graceful task termination
- Resource cleanup verification

### Configuration Management

#### Data Directory
- Automatic creation if non-existent
- Configurable location via global flags
- Proper error handling for access issues

#### Network Configuration
- Domain type inheritance from global config
- Network-specific parameter propagation
- Ethereum specification matching

### Security Considerations

#### Signal Safety
- Safe environment variable setting at startup
- Proper signal handler registration
- Race condition prevention

#### Error Information
- Sanitized error messages in logs
- No sensitive data in error output
- Proper error context preservation