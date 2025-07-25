# HTTP API Component - AI Documentation

## Overview
The HTTP API component provides a RESTful interface for the Anchor client, enabling external systems to query validator and committee information. This component serves as the primary API gateway for monitoring and management operations.

## Architecture Pattern
- **Pattern**: HTTP REST API Server with Axum framework
- **State Management**: Shared state through Arc<RwLock<Shared>> for thread-safe access
- **Database Integration**: Read-only access to NetworkState via watch channel receivers
- **Configuration**: Environment-driven configuration with sensible defaults

## Core Components

### 1. Configuration (`config.rs`)
- **Purpose**: HTTP server configuration management
- **Key Settings**: Listen address, port, CORS origin, enable/disable flag
- **Default Behavior**: Disabled by default, localhost binding on port 5062

### 2. Router (`router.rs`) 
- **Purpose**: HTTP route definitions and handler implementations
- **Endpoints**: Version, health, validators, committees
- **Response Format**: Consistent GenericResponse<T> wrapper for all endpoints

### 3. Server Context (`lib.rs`)
- **Purpose**: Server lifecycle management and dependency injection
- **State Sharing**: Thread-safe shared state for database access
- **Graceful Handling**: Handles optional/missing dependencies gracefully

## AI-Relevant Design Patterns

### State Management Pattern
```rust
pub struct Shared {
    pub database_state: Option<watch::Receiver<NetworkState>>,
}
```
- **Thread Safety**: Uses parking_lot::RwLock for efficient reader-writer access
- **Optional Dependencies**: Gracefully handles missing database connections
- **Watch Channel**: Uses tokio::sync::watch for reactive state updates

### Generic Response Pattern
All endpoints return `GenericResponse<T>` wrapper:
- **Consistency**: Uniform API response structure
- **Type Safety**: Compile-time guaranteed response types
- **Serialization**: Automatic JSON serialization via Serde

### Conditional Serving Pattern
```rust
if !config.enabled {
    info!("HTTP API Disabled");
    return Ok(());
}
```
- **Feature Toggling**: Runtime enable/disable capability
- **Resource Conservation**: No server resources consumed when disabled

## Integration Points

### Database Integration
- **Read-Only Access**: Safe concurrent access to validator/committee data
- **Reactive Updates**: Watch channel pattern for state synchronization
- **Fallback Behavior**: Returns empty collections when database unavailable

### Health Monitoring
- **Health Metrics**: Integration with health_metrics crate for system status
- **Observability**: Built-in health endpoint for monitoring systems

### Version Management
- **Platform Info**: Includes platform-specific version information
- **Build Metadata**: Provides version tracking for deployment management

## Performance Characteristics

### Async/Non-blocking
- **Framework**: Built on Axum (async-first HTTP framework)
- **Runtime**: Tokio async runtime for concurrent request handling
- **State Access**: Non-blocking reads via RwLock with minimal contention

### Memory Efficiency
- **Shared State**: Single shared state instance across all request handlers
- **Zero-Copy**: Direct access to database state without unnecessary cloning
- **Optional Features**: Only consumes resources when enabled

## Error Handling Strategy

### Graceful Degradation
- **Missing Database**: Returns empty collections instead of errors
- **Optional Fields**: Uses Option<T> for non-critical data
- **Service Availability**: Can run independently of other components

### Consistent Error Responses
- **Health Endpoint**: Returns Result<Health, String> wrapped in GenericResponse
- **Server Errors**: Proper HTTP status codes and error formatting

## Security Considerations

### Current State
- **TODO**: API endpoint protection not yet implemented
- **Local Binding**: Defaults to localhost for security
- **CORS**: Configurable origin restrictions

### Planned Security Features
- **API Secrets**: Authentication mechanism (marked as TODO)
- **Access Control**: Endpoint-level authorization (planned)

## Extensibility Points

### Adding New Endpoints
1. Add route in `router::new()`
2. Implement async handler function
3. Define response type in api_types crate
4. Ensure consistent GenericResponse wrapping

### Configuration Extensions
- **Network Settings**: Additional bind addresses, TLS configuration
- **Rate Limiting**: Request throttling configuration
- **Caching**: Response caching configuration

## Monitoring and Observability

### Built-in Endpoints
- **Health Check**: `/anchor/health` - System health status
- **Version Info**: `/anchor/version` - Build and platform information

### Logging
- **Tracing Integration**: Structured logging via tracing crate
- **Startup Logging**: Clear indication of server state (enabled/disabled)

## Dependencies and Relationships

### Core Dependencies
- **axum**: HTTP server framework
- **tokio**: Async runtime
- **serde/serde_json**: Serialization
- **parking_lot**: Efficient locking primitives

### Workspace Dependencies
- **api_types**: Response type definitions
- **database**: Network state access
- **health_metrics**: System health monitoring
- **ssv_types**: Committee and validator types

This component serves as a critical interface layer, providing external systems with structured access to the Anchor client's internal state while maintaining performance and reliability through well-established async patterns.