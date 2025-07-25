# HTTP API Usage Examples

## Configuration Examples

### Basic Configuration
```rust
use http_api::Config;
use std::net::{IpAddr, Ipv4Addr};

// Default configuration (disabled)
let default_config = Config::default();
assert_eq!(default_config.enabled, false);
assert_eq!(default_config.listen_port, 5062);

// Enable the HTTP API
let enabled_config = Config {
    enabled: true,
    listen_addr: IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), // Bind to all interfaces
    listen_port: 8080,
    allow_origin: Some("http://localhost:3000".to_string()),
};
```

### Production Configuration
```rust
use http_api::Config;
use std::net::{IpAddr, Ipv4Addr};

let production_config = Config {
    enabled: true,
    listen_addr: IpAddr::V4(Ipv4Addr::new(10, 0, 0, 100)), // Internal network
    listen_port: 5062,
    allow_origin: Some("https://dashboard.example.com".to_string()),
};
```

## Server Setup Examples

### Basic Server Startup
```rust
use http_api::{run, Config, Shared};
use parking_lot::RwLock;
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<(), String> {
    let config = Config {
        enabled: true,
        ..Default::default()
    };
    
    let shared_state = Arc::new(RwLock::new(Shared {
        database_state: None, // No database connection
    }));
    
    // Start the HTTP API server
    run(config, shared_state).await?;
    Ok(())
}
```

### Server with Database Integration
```rust
use http_api::{run, Config, Shared};
use parking_lot::RwLock;
use std::sync::Arc;
use tokio::sync::watch;
use database::NetworkState;

#[tokio::main]
async fn main() -> Result<(), String> {
    // Setup database state receiver
    let (tx, rx) = watch::channel(NetworkState::default());
    
    let config = Config {
        enabled: true,
        listen_addr: "127.0.0.1".parse().unwrap(),
        listen_port: 5062,
        allow_origin: None,
    };
    
    let shared_state = Arc::new(RwLock::new(Shared {
        database_state: Some(rx),
    }));
    
    // Start the HTTP API server with database connection
    run(config, shared_state).await?;
    Ok(())
}
```

## HTTP Client Examples

### Using curl

#### Get Version Information
```bash
curl -X GET http://localhost:5062/anchor/version
```

**Response:**
```json
{
  "data": {
    "version": "anchor/v1.0.0-linux-x86_64"
  }
}
```

#### Get Health Status
```bash
curl -X GET http://localhost:5062/anchor/health
```

**Response:**
```json
{
  "data": {
    "status": "ok",
    "uptime": 3600,
    "memory_usage": "256MB"
  }
}
```

#### Get Validators
```bash
curl -X GET http://localhost:5062/anchor/validators
```

**Response:**
```json
{
  "data": [
    {
      "public_key": "0x8d9c9c9c9c9c9c9c9c9c9c9c9c9c9c9c9c9c9c9c9c9c9c9c9c9c9c9c9c9c9c9c",
      "cluster_id": "ClusterId(123)",
      "index": 42,
      "graffiti": "416e63686f72"
    },
    {
      "public_key": "0x7a8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b8b",
      "cluster_id": "ClusterId(456)",
      "index": null,
      "graffiti": "56616c696461746f72"
    }
  ]
}
```

#### Get Committees
```bash
curl -X GET http://localhost:5062/anchor/committees
```

**Response:**
```json
{
  "data": [
    {
      "committee_id": "CommitteeId(789)",
      "committee_members": [1, 2, 3, 4],
      "validator_indices": [42, 43, 44, 45]
    }
  ]
}
```

### Using HTTP Client Libraries

#### Python with requests
```python
import requests
import json

# Base URL for the API
BASE_URL = "http://localhost:5062"

def get_version():
    response = requests.get(f"{BASE_URL}/anchor/version")
    return response.json()

def get_validators():
    response = requests.get(f"{BASE_URL}/anchor/validators")
    return response.json()

def get_committees():
    response = requests.get(f"{BASE_URL}/anchor/committees")
    return response.json()

def get_health():
    response = requests.get(f"{BASE_URL}/anchor/health")
    return response.json()

# Usage examples
if __name__ == "__main__":
    print("Version:", get_version())
    print("Validators:", get_validators())
    print("Committees:", get_committees())
    print("Health:", get_health())
```

#### JavaScript with fetch
```javascript
const BASE_URL = 'http://localhost:5062';

async function getVersion() {
  const response = await fetch(`${BASE_URL}/anchor/version`);
  return await response.json();
}

async function getValidators() {
  const response = await fetch(`${BASE_URL}/anchor/validators`);
  return await response.json();
}

async function getCommittees() {
  const response = await fetch(`${BASE_URL}/anchor/committees`);
  return await response.json();
}

async function getHealth() {
  const response = await fetch(`${BASE_URL}/anchor/health`);
  return await response.json();
}

// Usage examples
async function main() {
  try {
    const version = await getVersion();
    console.log('Version:', version);
    
    const validators = await getValidators();
    console.log('Validators:', validators);
    
    const committees = await getCommittees();
    console.log('Committees:', committees);
    
    const health = await getHealth();
    console.log('Health:', health);
  } catch (error) {
    console.error('API Error:', error);
  }
}

main();
```

#### Rust with reqwest
```rust
use reqwest;
use serde_json::Value;

const BASE_URL: &str = "http://localhost:5062";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = reqwest::Client::new();
    
    // Get version
    let version_response = client
        .get(&format!("{}/anchor/version", BASE_URL))
        .send()
        .await?;
    let version: Value = version_response.json().await?;
    println!("Version: {}", serde_json::to_string_pretty(&version)?);
    
    // Get validators
    let validators_response = client
        .get(&format!("{}/anchor/validators", BASE_URL))
        .send()
        .await?;
    let validators: Value = validators_response.json().await?;
    println!("Validators: {}", serde_json::to_string_pretty(&validators)?);
    
    // Get committees
    let committees_response = client
        .get(&format!("{}/anchor/committees", BASE_URL))
        .send()
        .await?;
    let committees: Value = committees_response.json().await?;
    println!("Committees: {}", serde_json::to_string_pretty(&committees)?);
    
    // Get health
    let health_response = client
        .get(&format!("{}/anchor/health", BASE_URL))
        .send()
        .await?;
    let health: Value = health_response.json().await?;
    println!("Health: {}", serde_json::to_string_pretty(&health)?);
    
    Ok(())
}
```

## Integration Examples

### Docker Health Check
```dockerfile
FROM ubuntu:20.04

# Install curl for health checks
RUN apt-get update && apt-get install -y curl

# Health check using the API
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
  CMD curl -f http://localhost:5062/anchor/health || exit 1

# Your application setup...
```

### Kubernetes Probes
```yaml
apiVersion: v1
kind: Pod
metadata:
  name: anchor-node
spec:
  containers:
  - name: anchor
    image: anchor:latest
    ports:
    - containerPort: 5062
    livenessProbe:
      httpGet:
        path: /anchor/health
        port: 5062
      initialDelaySeconds: 30
      periodSeconds: 10
    readinessProbe:
      httpGet:
        path: /anchor/version
        port: 5062
      initialDelaySeconds: 5
      periodSeconds: 5
```

### Monitoring Integration (Prometheus)
```yaml
# prometheus.yml
global:
  scrape_interval: 15s

scrape_configs:
  - job_name: 'anchor-api'
    static_configs:
      - targets: ['localhost:5062']
    metrics_path: '/anchor/health'
    scrape_interval: 30s
```

### Load Balancer Configuration (nginx)
```nginx
upstream anchor_api {
    server 127.0.0.1:5062;
    # Add more instances for load balancing
    # server 127.0.0.1:5063;
}

server {
    listen 80;
    server_name api.anchor.example.com;

    location /anchor/ {
        proxy_pass http://anchor_api/anchor/;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
        
        # CORS headers
        add_header Access-Control-Allow-Origin *;
        add_header Access-Control-Allow-Methods "GET, OPTIONS";
        add_header Access-Control-Allow-Headers "Origin, X-Requested-With, Content-Type, Accept";
    }
    
    # Health check endpoint
    location /health {
        proxy_pass http://anchor_api/anchor/health;
    }
}
```

## Error Handling Examples

### Network Errors
```rust
use reqwest;

async fn fetch_validators() -> Result<Vec<ValidatorData>, String> {
    let client = reqwest::Client::new();
    
    match client.get("http://localhost:5062/anchor/validators").send().await {
        Ok(response) => {
            if response.status().is_success() {
                match response.json().await {
                    Ok(data) => Ok(data),
                    Err(e) => Err(format!("Failed to parse JSON: {}", e)),
                }
            } else {
                Err(format!("HTTP error: {}", response.status()))
            }
        }
        Err(e) => Err(format!("Network error: {}", e)),
    }
}
```

### Timeout Handling
```python
import requests
from requests.adapters import HTTPAdapter
from requests.packages.urllib3.util.retry import Retry

def create_session():
    session = requests.Session()
    retry = Retry(
        total=3,
        read=3,
        connect=3,
        backoff_factor=0.3,
        status_forcelist=(500, 502, 504)
    )
    adapter = HTTPAdapter(max_retries=retry)
    session.mount('http://', adapter)
    session.mount('https://', adapter)
    return session

def get_validators_with_timeout():
    session = create_session()
    try:
        response = session.get(
            'http://localhost:5062/anchor/validators',
            timeout=10  # 10 second timeout
        )
        return response.json()
    except requests.exceptions.Timeout:
        print("Request timed out")
        return None
    except requests.exceptions.RequestException as e:
        print(f"Request failed: {e}")
        return None
```

## Performance Optimization Examples

### Connection Pooling
```rust
use reqwest::Client;
use std::time::Duration;

// Create a client with connection pooling
let client = Client::builder()
    .pool_max_idle_per_host(10)
    .pool_idle_timeout(Duration::from_secs(30))
    .timeout(Duration::from_secs(10))
    .build()?;

// Reuse the same client for multiple requests
let version = client.get("http://localhost:5062/anchor/version").send().await?;
let health = client.get("http://localhost:5062/anchor/health").send().await?;
```

### Concurrent Requests
```rust
use tokio;

async fn fetch_all_data() -> Result<(), Box<dyn std::error::Error>> {
    let client = reqwest::Client::new();
    
    // Make concurrent requests
    let (version_result, validators_result, committees_result) = tokio::join!(
        client.get("http://localhost:5062/anchor/version").send(),
        client.get("http://localhost:5062/anchor/validators").send(),
        client.get("http://localhost:5062/anchor/committees").send()
    );
    
    let version = version_result?.json().await?;
    let validators = validators_result?.json().await?;
    let committees = committees_result?.json().await?;
    
    // Process results...
    Ok(())
}
```

These examples demonstrate various ways to configure, deploy, and interact with the HTTP API component in different environments and use cases.