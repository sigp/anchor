# HTTP API Component Specification

## Component Identity
- **Name**: HTTP API
- **Crate**: `http_api`
- **Version**: 0.1.0
- **Authors**: Sigma Prime <contact@sigmaprime.io>

## Purpose Statement
Provides a RESTful HTTP interface for external systems to query Anchor client state, including validator information, committee data, health status, and version information.

## Technical Architecture

### Module Structure
```
src/
├── lib.rs          # Main module with server context and runtime
├── config.rs       # Configuration structures and defaults
└── router.rs       # HTTP route definitions and handlers
```

### Core Types

#### Configuration
```rust
pub struct Config {
    pub enabled: bool,                    // Server enable/disable flag
    pub listen_addr: IpAddr,             // Bind address (default: 127.0.0.1)
    pub listen_port: u16,                // Listen port (default: 5062)
    pub allow_origin: Option<String>,     // CORS origin header
}
```

#### Server Context
```rust
pub struct Context<T: SlotClock> {
    pub task_executor: TaskExecutor,
    pub secrets_dir: Option<PathBuf>,
    pub config: Config,
    pub slot_clock: T,
}

pub struct Shared {
    pub database_state: Option<watch::Receiver<NetworkState>>,
}
```

### API Endpoints

#### GET /
- **Response**: `"Anchor client"` (static string)
- **Purpose**: Basic connectivity test

#### GET /anchor/version
- **Response**: `GenericResponse<VersionData>`
- **Data Structure**:
  ```rust
  {
    "data": {
      "version": "string"  // Platform-specific version info
    }
  }
  ```

#### GET /anchor/health
- **Response**: `GenericResponse<Result<Health, String>>`
- **Data Structure**:
  ```rust
  {
    "data": {
      // Health status from health_metrics::observe::Observe
    }
  }
  ```

#### GET /anchor/validators
- **Response**: `GenericResponse<Vec<ValidatorData>>`
- **Data Structure**:
  ```rust
  {
    "data": [
      {
        "public_key": "string",      // Validator public key
        "cluster_id": "string",      // Debug-formatted cluster ID
        "index": number | null,      // Optional validator index
        "graffiti": "string"         // Hex-encoded graffiti
      }
    ]
  }
  ```

#### GET /anchor/committees
- **Response**: `GenericResponse<Vec<CommitteeData>>`
- **Data Structure**:
  ```rust
  {
    "data": [
      {
        "committee_id": "string",           // Debug-formatted committee ID
        "committee_members": [number],      // Array of operator IDs (u64)
        "validator_indices": [number]       // Array of validator indices (usize)
      }
    ]
  }
  ```

## Dependencies

### Internal Dependencies
```toml
api_types = { workspace = true }        # Response type definitions
database = { workspace = true }         # Network state access
health_metrics = { workspace = true }   # Health monitoring
slot_clock = { workspace = true }       # Time synchronization
ssv_types = { workspace = true }        # SSV protocol types
task_executor = { workspace = true }    # Task execution framework
types = { workspace = true }            # Common type definitions
version = { workspace = true }          # Version information
```

### External Dependencies
```toml
axum = { workspace = true }             # HTTP server framework
hex = { workspace = true }              # Hex encoding utilities
parking_lot = { workspace = true }      # Efficient synchronization primitives
serde = { workspace = true }            # Serialization framework
serde_json = { workspace = true }       # JSON serialization
tokio = { workspace = true }            # Async runtime
tracing = { workspace = true }          # Structured logging
```

## Runtime Behavior

### Server Lifecycle
1. **Configuration Check**: Verify `config.enabled` flag
2. **Route Setup**: Initialize Axum router with all endpoints
3. **Socket Binding**: Bind to configured address and port
4. **Server Start**: Launch async HTTP server
5. **Request Handling**: Process incoming HTTP requests concurrently

### State Management
- **Shared State**: `Arc<RwLock<Shared>>` passed to all route handlers
- **Database Access**: Optional `watch::Receiver<NetworkState>` for reactive updates
- **Thread Safety**: RwLock ensures safe concurrent access
- **Graceful Degradation**: Returns empty collections when database unavailable

### Error Handling
- **Server Disabled**: Clean exit with info log when `enabled = false`
- **Bind Errors**: Return descriptive error strings
- **Missing Data**: Return empty collections instead of errors
- **Serialization**: Automatic JSON error responses via Axum

## Performance Characteristics

### Concurrency Model
- **Async/Await**: Non-blocking request processing
- **Shared State**: Minimal lock contention with RwLock
- **Zero-Copy**: Direct access to database state without cloning

### Memory Usage
- **Static Routes**: Compile-time route definitions
- **Shared Context**: Single state instance across all handlers
- **Efficient Serialization**: Direct JSON serialization without intermediate allocations

### Network Performance
- **HTTP/1.1**: Standard HTTP protocol support
- **Keep-Alive**: Connection reuse for better performance
- **Compression**: Automatic response compression via Axum middleware

## Configuration Specification

### Default Configuration
```rust
Config {
    enabled: false,                                    // Disabled by default
    listen_addr: IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), // localhost
    listen_port: 5062,                                // Port 5062
    allow_origin: None,                               // No CORS by default
}
```

### Configuration Sources
- **Default Values**: Compile-time defaults for all fields
- **External Config**: Integration with workspace configuration system
- **Environment Variables**: (Implementation dependent on parent application)

## Security Model

### Current Security Features
- **Local Binding**: Default localhost binding prevents external access
- **CORS Control**: Optional origin header restriction
- **Read-Only Access**: No state modification endpoints

### Security Limitations
- **No Authentication**: API endpoints are currently unprotected (TODO marked)
- **No Rate Limiting**: No request throttling implemented
- **No TLS**: HTTP-only communication (no HTTPS support)

### Planned Security Enhancements
- **API Secrets**: Authentication mechanism (marked as TODO in code)
- **Endpoint Protection**: Access control for sensitive endpoints

## Integration Requirements

### Database Integration
- **NetworkState Access**: Requires database component with NetworkState
- **Watch Channel**: Uses tokio::sync::watch for state updates
- **Optional Dependency**: Gracefully handles missing database connection

### Health Monitoring Integration
- **Health Metrics**: Requires health_metrics crate implementation
- **Observability**: Must implement Observe trait for health checks

### Task Executor Integration
- **Context Requirement**: Requires TaskExecutor for server context
- **Async Runtime**: Must run within tokio runtime environment

## Testing Considerations

### Unit Testing
- **Route Testing**: Test individual endpoint handlers
- **Configuration Testing**: Verify default and custom configurations
- **State Management**: Test shared state access patterns

### Integration Testing
- **Server Lifecycle**: Test startup, shutdown, and error conditions
- **Endpoint Responses**: Verify complete request-response cycles
- **Database Integration**: Test with and without database state

### Performance Testing
- **Concurrent Requests**: Test multiple simultaneous requests
- **Memory Usage**: Monitor memory consumption under load
- **Response Times**: Measure endpoint response latencies

## Monitoring and Observability

### Logging Points
- **Server Status**: Startup/shutdown events
- **Configuration**: Enabled/disabled state
- **Error Conditions**: Bind failures, runtime errors

### Metrics Collection
- **Health Endpoint**: Built-in health status reporting
- **Version Endpoint**: Build and deployment tracking
- **Request Metrics**: (Implementation dependent on parent application)

## Future Enhancement Points

### Protocol Extensions
- **WebSocket Support**: Real-time state updates
- **GraphQL**: More flexible query capabilities
- **gRPC**: High-performance binary protocol option

### Security Enhancements
- **TLS/HTTPS**: Encrypted communication
- **JWT Authentication**: Token-based authentication
- **Rate Limiting**: Request throttling and abuse prevention

### Operational Features
- **Metrics Export**: Prometheus/OpenTelemetry integration
- **Configuration Reload**: Runtime configuration updates
- **Graceful Shutdown**: Clean connection termination