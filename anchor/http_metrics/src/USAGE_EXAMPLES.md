# HTTP Metrics Component - Usage Examples

## Basic Integration Example

### Client Integration Pattern
```rust
use http_metrics::{Config, Shared, serve};
use std::sync::Arc;
use parking_lot::RwLock;
use tokio::net::TcpListener;

// 1. Create configuration
let config = Config {
    enabled: true,
    listen_addr: "127.0.0.1".parse().unwrap(),
    listen_port: 5164,
    allow_origin: None,
};

// 2. Create shared state
let shared_state = Arc::new(RwLock::new(Shared {
    genesis_time: None,
    duties_service: None, 
    network_registry: None,
}));

// 3. Start server (conditional on enabled flag)
if config.enabled {
    let socket_addr = SocketAddr::new(config.listen_addr, config.listen_port);
    let listener = TcpListener::bind(socket_addr).await
        .expect("Failed to bind metrics server");
    
    let shared_clone = shared_state.clone();
    let shutdown_signal = async {
        // Your shutdown logic here
        tokio::signal::ctrl_c().await.unwrap();
    };
    
    tokio::spawn(async move {
        serve(listener, shared_clone, shutdown_signal).await;
    });
}
```

## Configuration Examples

### Default Configuration
```rust
use http_metrics::Config;

// Using default values
let config = Config::default();
assert_eq!(config.enabled, false);
assert_eq!(config.listen_port, 5164);
assert_eq!(config.listen_addr, "127.0.0.1".parse().unwrap());
```

### Custom Configuration
```rust
use http_metrics::Config;
use std::net::IpAddr;

let config = Config {
    enabled: true,
    listen_addr: IpAddr::V4("0.0.0.0".parse().unwrap()), // Bind to all interfaces
    listen_port: 8080,
    allow_origin: Some("https://grafana.example.com".to_string()),
};
```

### CLI Configuration Mapping
```rust
// In client/src/config.rs - CLI argument handling
pub fn from_cli(cli_args: &ArgMatches, mut config: Config) -> Result<Config> {
    if cli_args.get_flag("metrics") {
        config.http_metrics.enabled = true;
    }
    
    if let Some(addr) = cli_args.get_one::<String>("metrics-address") {
        config.http_metrics.listen_addr = addr.parse()
            .map_err(|_| "Invalid metrics address")?;
    }
    
    if let Some(port) = cli_args.get_one::<u16>("metrics-port") {
        config.http_metrics.listen_port = *port;
    }
    
    Ok(config)
}
```

## State Management Examples

### Updating Genesis Time
```rust
use http_metrics::Shared;
use std::time::{SystemTime, UNIX_EPOCH};

// When genesis time becomes known
let genesis_timestamp = 1606824023u64; // Example genesis time
{
    let mut shared = shared_state.write();
    shared.genesis_time = Some(genesis_timestamp);
}
```

### Adding Duties Service
```rust
use validator_services::duties_service::DutiesService;

// When duties service is initialized
let duties_service = Arc::new(DutiesService::new(/* params */));
{
    let mut shared = shared_state.write();
    shared.duties_service = Some(duties_service);
}
```

### Adding Network Registry
```rust
use lighthouse_network::libp2p::metrics::Registry;

// When network layer is initialized
let network_registry = Registry::new();
{
    let mut shared = shared_state.write();
    shared.network_registry = Some(network_registry);
}
```

## HTTP Client Examples

### Basic Metrics Request
```bash
# Fetch metrics using curl
curl http://localhost:5164/metrics
```

### Prometheus Configuration
```yaml
# prometheus.yml
global:
  scrape_interval: 15s

scrape_configs:
  - job_name: 'anchor-validator'
    static_configs:
      - targets: ['localhost:5164']
    scrape_interval: 10s
    metrics_path: /metrics
```

### Grafana Dashboard Query
```promql
# Genesis distance (seconds since genesis)
genesis_distance

# Validator duty counts
proposer_count{epoch="current"}
attester_count{epoch="current"}
attester_count{epoch="next"}

# Rate of duties over time
rate(proposer_count[5m])
```

## Testing Examples

### Unit Test Setup
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;
    use std::time::Duration;

    #[tokio::test]
    async fn test_metrics_server_startup() {
        let shared_state = Arc::new(RwLock::new(Shared {
            genesis_time: Some(1606824023),
            duties_service: None,
            network_registry: None,
        }));

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let shutdown = async {
            tokio::time::sleep(Duration::from_millis(100)).await;
        };

        tokio::spawn(async move {
            serve(listener, shared_state, shutdown).await;
        });

        // Test metrics endpoint
        let response = reqwest::get(&format!("http://{}/metrics", addr))
            .await
            .unwrap();
        
        assert_eq!(response.status(), 200);
        let body = response.text().await.unwrap();
        assert!(body.contains("genesis_distance"));
    }
}
```

### Integration Test with Mock Data
```rust
#[tokio::test]
async fn test_metrics_with_duties_service() {
    let duties_service = Arc::new(MockDutiesService::new());
    let shared_state = Arc::new(RwLock::new(Shared {
        genesis_time: Some(1606824023),
        duties_service: Some(duties_service),
        network_registry: None,
    }));

    // Start server and test metrics contain duty counts
    // ... server setup ...

    let response = reqwest::get(&format!("http://{}/metrics", addr))
        .await
        .unwrap();
    
    let body = response.text().await.unwrap();
    assert!(body.contains("proposer_count"));
    assert!(body.contains("attester_count"));
}
```

## Deployment Examples

### Docker Configuration
```dockerfile
# Expose metrics port
EXPOSE 5164

# Run with metrics enabled
CMD ["anchor", "--metrics", "--metrics-address", "0.0.0.0"]
```

### Systemd Service
```ini
[Unit]
Description=Anchor Validator with Metrics
After=network.target

[Service]
Type=simple
User=anchor
ExecStart=/usr/local/bin/anchor \
  --metrics \
  --metrics-port 5164 \
  --config /etc/anchor/config.yaml
Restart=always

[Install]
WantedBy=multi-user.target
```

### Kubernetes Deployment
```yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: anchor-validator
spec:
  replicas: 1
  selector:
    matchLabels:
      app: anchor-validator
  template:
    metadata:
      labels:
        app: anchor-validator
    spec:
      containers:
      - name: anchor
        image: anchor:latest
        args: ["--metrics", "--metrics-address", "0.0.0.0"]
        ports:
        - containerPort: 5164
          name: metrics
---
apiVersion: v1
kind: Service
metadata:
  name: anchor-metrics
  labels:
    app: anchor-validator
spec:
  ports:
  - port: 5164
    name: metrics
  selector:
    app: anchor-validator
```

## Error Handling Examples

### Network Binding Errors
```rust
async fn start_metrics_server(config: &Config) -> Result<(), Box<dyn std::error::Error>> {
    if !config.http_metrics.enabled {
        return Ok(());
    }

    let socket_addr = SocketAddr::new(
        config.http_metrics.listen_addr, 
        config.http_metrics.listen_port
    );

    match TcpListener::bind(socket_addr).await {
        Ok(listener) => {
            println!("Metrics server listening on {}", socket_addr);
            // Start server...
            Ok(())
        }
        Err(e) => {
            eprintln!("Failed to bind metrics server to {}: {}", socket_addr, e);
            Err(e.into())
        }
    }
}
```

### Prometheus Encoding Errors
```rust
// Internal error handling in metrics_handler
if let Err(e) = encoder.encode_utf8(&gather(), &mut buffer) {
    return (
        StatusCode::INTERNAL_SERVER_ERROR,
        format!("Failed to encode prometheus data: {e}"),
    ).into_response();
}
```

## Monitoring Integration Examples

### Alerting Rules
```yaml
# prometheus-alerts.yml
groups:
- name: anchor.rules
  rules:
  - alert: AnchorValidatorDown
    expr: up{job="anchor-validator"} == 0
    for: 1m
    labels:
      severity: critical
    annotations:
      summary: "Anchor validator is down"
      
  - alert: LowAttestationRate
    expr: rate(attester_count[5m]) < 0.8
    for: 2m
    labels:
      severity: warning
    annotations:
      summary: "Low attestation rate detected"
```

### Custom Metrics Collection
```rust
// Example of extending metrics handler for custom metrics
async fn extended_metrics_handler<E: EthSpec>(
    State(state): State<Arc<RwLock<Shared<E>>>>,
) -> Response<Body> {
    let mut buffer = String::new();
    
    // Standard metrics collection
    let shared = state.read();
    
    // Custom application metrics
    if let Some(duties_service) = &shared.duties_service {
        let success_rate = duties_service.calculate_success_rate();
        writeln!(buffer, "# HELP duty_success_rate Success rate of validator duties").unwrap();
        writeln!(buffer, "# TYPE duty_success_rate gauge").unwrap();
        writeln!(buffer, "duty_success_rate {}", success_rate).unwrap();
    }
    
    buffer.into_response()
}
```

## Best Practices

### Performance Optimization
```rust
// Minimize lock contention by reading state once
async fn optimized_metrics_handler<E: EthSpec>(
    State(state): State<Arc<RwLock<Shared<E>>>>,
) -> Response<Body> {
    // Take snapshot of shared state
    let (genesis_time, duties_service, network_registry) = {
        let shared = state.read();
        (
            shared.genesis_time,
            shared.duties_service.clone(),
            shared.network_registry.clone(),
        )
    };
    
    // Process metrics without holding lock
    let mut buffer = String::new();
    if let Some(genesis) = genesis_time {
        // Process genesis metrics...
    }
    
    buffer.into_response()
}
```

### Resource Management
```rust
// Graceful server shutdown with timeout
async fn shutdown_metrics_server(
    server_handle: tokio::task::JoinHandle<()>
) -> Result<(), tokio::task::JoinError> {
    // Send shutdown signal
    tokio::select! {
        result = server_handle => result,
        _ = tokio::time::sleep(Duration::from_secs(30)) => {
            eprintln!("Metrics server shutdown timed out");
            Err(tokio::task::JoinError::from(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "Server shutdown timeout"
            )))
        }
    }
}
```