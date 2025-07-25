# HTTP Metrics Component - Technical Specification

## Component Details

**Crate Name**: `http_metrics`  
**Version**: `0.1.0`  
**Location**: `anchor/http_metrics/src/`  
**Authors**: Sigma Prime <contact@sigmaprime.io>

## Public API

### Core Types

#### `Shared<E: EthSpec>`
```rust
pub struct Shared<E: EthSpec> {
    pub genesis_time: Option<u64>,
    pub duties_service: Option<Arc<DutiesService<ValidatorStore<E>, SystemTimeSlotClock>>>,
    pub network_registry: Option<Registry>,
}
```

**Purpose**: Thread-safe container for shared state between main client and metrics server.

**Fields**:
- `genesis_time`: Unix timestamp of blockchain genesis (populated once known)
- `duties_service`: Reference to validator duties tracking service
- `network_registry`: Prometheus metrics registry for network layer metrics

#### `Config`
```rust
#[derive(PartialEq, Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub enabled: bool,
    pub listen_addr: IpAddr,
    pub listen_port: u16,
    pub allow_origin: Option<String>,
}
```

**Purpose**: Configuration for the HTTP metrics server.

**Fields**:
- `enabled`: Feature toggle for metrics server (default: `false`)
- `listen_addr`: IP address to bind server (default: `127.0.0.1`)
- `listen_port`: TCP port to bind server (default: `5164`)
- `allow_origin`: CORS origin configuration (default: `None`)

**Default Implementation**:
```rust
impl Default for Config {
    fn default() -> Self {
        Self {
            enabled: false,
            listen_addr: IpAddr::V4(Ipv4Addr::LOCALHOST),
            listen_port: 5164,
            allow_origin: None,
        }
    }
}
```

### Core Functions

#### `serve<E: EthSpec>`
```rust
pub async fn serve<E: EthSpec>(
    listener: TcpListener,
    shared_state: Arc<RwLock<Shared<E>>>,
    shutdown: impl Future<Output = ()> + Send + Sync + 'static,
)
```

**Purpose**: Creates and runs the HTTP metrics server.

**Parameters**:
- `listener`: Pre-bound TCP listener for the server
- `shared_state`: Shared state container for metrics data
- `shutdown`: Future that resolves when server should gracefully shutdown

**Behavior**:
- Creates Axum router with `/metrics` endpoint
- Configures CORS middleware (allows GET/POST from any origin)
- Runs server with graceful shutdown capability
- Logs errors if server fails

#### `create_router<E: EthSpec>` (Private)
```rust
fn create_router<E: EthSpec>(shared_state: Arc<RwLock<Shared<E>>>) -> Router
```

**Purpose**: Creates the Axum router with CORS configuration.

**Routes**:
- `GET /metrics`: Prometheus metrics endpoint

#### `metrics_handler<E: EthSpec>` (Private)
```rust
async fn metrics_handler<E: EthSpec>(
    State(state): State<Arc<RwLock<Shared<E>>>>,
) -> Response<Body>
```

**Purpose**: Handles requests to the `/metrics` endpoint.

**Response Format**: Prometheus text exposition format

## Dependencies

### External Dependencies
```toml
anchor_validator_store = { workspace = true }
axum = { workspace = true }
health_metrics = { workspace = true }
lighthouse_network = { workspace = true }
metrics = { workspace = true }
parking_lot = { workspace = true }
serde = { workspace = true }
slot_clock = { workspace = true }
tokio = { workspace = true }
tower-http = { workspace = true }
tracing = { workspace = true }
types = { workspace = true }
validator_metrics = { workspace = true }
validator_services = { workspace = true }
```

### Key Dependency Functions
- `lighthouse_network::prometheus_client::encoding::text::encode`: Prometheus encoding
- `validator_metrics::*`: Standard validator metrics
- `health_metrics::metrics::scrape_health_metrics()`: Health metrics collection
- `lighthouse_network::metrics::scrape_discovery_metrics()`: Discovery metrics

## HTTP Endpoints

### `GET /metrics`

**Purpose**: Returns Prometheus-formatted metrics data

**Response Headers**:
- `Content-Type`: `text/plain; version=0.0.4; charset=utf-8`
- CORS headers (Access-Control-Allow-Origin, etc.)

**Response Body**: Prometheus text exposition format containing:

#### Validator Metrics
- `genesis_distance`: Seconds since genesis time
- `proposer_count{epoch="current"}`: Number of proposer duties for current epoch
- `attester_count{epoch="current"}`: Number of attester duties for current epoch  
- `attester_count{epoch="next"}`: Number of attester duties for next epoch

#### Network Metrics
- libp2p connection metrics
- Peer discovery metrics
- Network health indicators

#### System Metrics
- Health check status
- Discovery service metrics

**Error Responses**:
- `500 Internal Server Error`: If Prometheus encoding fails

**Example Response**:
```
# HELP genesis_distance Seconds since genesis
# TYPE genesis_distance gauge
genesis_distance 1234567

# HELP proposer_count Number of proposer duties
# TYPE proposer_count gauge
proposer_count{epoch="current"} 5

# HELP attester_count Number of attester duties  
# TYPE attester_count gauge
attester_count{epoch="current"} 100
attester_count{epoch="next"} 95
```

## Data Flow Specification

### Initialization Sequence
1. Main client creates `Shared<E>` with `None` values
2. Client wraps in `Arc<RwLock<>>` for thread safety
3. TCP listener bound to configured address:port
4. `serve()` function spawned as background task
5. Server starts accepting connections

### Runtime Data Updates
1. **Genesis Time**: Set once when beacon chain genesis is known
2. **Duties Service**: Set when duties service is initialized
3. **Network Registry**: Set when network layer is initialized

### Request Processing
1. Client sends `GET /metrics`
2. Handler acquires read lock on shared state
3. Conditional metric collection based on available data:
   - Genesis distance (if genesis_time available)
   - Validator duties (if duties_service available)
   - Network metrics (if network_registry available)
4. Additional metric scraping (health, discovery)
5. Prometheus encoding to string buffer
6. Return formatted response

## Threading & Concurrency

### Thread Safety
- `Arc<RwLock<Shared<E>>>` provides thread-safe access
- Read-heavy workload (metrics serving) vs write-light (state updates)
- RwLock allows concurrent reads during metric serving

### Async Runtime
- Built on Tokio async runtime
- Axum server handles requests asynchronously
- Non-blocking I/O for HTTP operations

## Error Handling Specification

### Server Startup Errors
- TCP binding failures logged and propagated to caller
- Server configuration errors handled at client level

### Runtime Errors
- Prometheus encoding errors: Return HTTP 500 with error message
- Missing shared state data: Gracefully skip unavailable metrics
- Network encoding errors: Return HTTP 500 with error message

### Graceful Shutdown
- Server responds to shutdown signal
- Existing connections allowed to complete
- Clean resource cleanup

## Performance Characteristics

### Memory Usage
- Minimal heap allocation (metrics collected on-demand)
- Shared state kept minimal (references only)
- String buffer allocation per request

### CPU Usage
- Lazy metric collection (only on HTTP request)
- Read lock acquisition overhead minimal
- Prometheus encoding computational cost

### Network
- Single TCP socket binding
- HTTP keep-alive supported via Axum
- CORS preflight handling

## Configuration Integration

### CLI Arguments
Mapped from client CLI to `Config` struct:
- `--metrics` → `enabled: true`
- `--metrics-address <IP>` → `listen_addr: <IP>`
- `--metrics-port <PORT>` → `listen_port: <PORT>`

### Environment Variables
None directly supported (handled at client configuration level)

## Monitoring & Observability

### Internal Logging
- Server startup/shutdown events logged via `tracing`
- Error conditions logged with context
- No debug logging in production paths

### Health Checking
- Server availability inherent in HTTP response capability
- Metrics themselves provide health indicators
- Integration with `health_metrics` crate

## Security Considerations

### Network Security
- Default binding to localhost only
- CORS configured to allow any origin (intended for monitoring tools)
- No authentication/authorization (metrics are non-sensitive operational data)

### Data Exposure
- Exposes validator count information
- Network peer information
- System timing information
- No private keys or sensitive validator data