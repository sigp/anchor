# Anchor Binary Usage Examples

## Basic Usage Patterns

### Starting an SSV Node

#### Basic Node Startup
```bash
# Start node with default configuration
anchor node

# Start node with custom data directory
anchor --data-dir /path/to/data node

# Start node with debug logging
anchor --debug-level debug node
```

#### Node with Custom Configuration
```bash
# Start node with specific network configuration
anchor --data-dir ./data --debug-level info node \
  --network mainnet \
  --eth1-endpoint http://localhost:8545 \
  --beacon-node-endpoint http://localhost:5052
```

### Key Management Operations

#### Generating New Keys
```bash
# Generate a new BLS keypair
anchor keygen --output-dir ./keys

# Generate keys with custom parameters
anchor --data-dir ./data keygen \
  --output-dir ./validator-keys \
  --password-file ./keystore-password.txt
```

#### Key Splitting for Distributed Validation
```bash
# Split an existing key for 4 operators with threshold of 3
anchor keysplit \
  --keystore ./validator.json \
  --password ./password.txt \
  --operators 4 \
  --threshold 3 \
  --output-dir ./split-keys

# Split with custom operator configurations
anchor keysplit \
  --keystore ./validator.json \
  --password ./password.txt \
  --operators-config ./operators.json \
  --output-dir ./distributed-keys
```

## Advanced Configuration Examples

### Logging Configuration

#### File Logging Setup
```bash
# Enable file logging with rotation
anchor node \
  --logfile-dir ./logs \
  --logfile-max-size 100MB \
  --logfile-debug-level debug \
  --logfile-color true
```

#### Environment-based Logging
```bash
# Set logging via environment variables
export RUST_LOG="anchor=debug,network=info,processor=warn"
anchor node

# Component-specific logging
export RUST_LOG="anchor::processor=debug,libp2p=warn"
anchor node --logfile-dir ./logs
```

### Network Configuration Examples

#### Mainnet Deployment
```bash
anchor node \
  --network mainnet \
  --data-dir /var/lib/anchor \
  --eth1-endpoint http://geth:8545 \
  --beacon-node-endpoint http://lighthouse:5052 \
  --p2p-port 13000 \
  --discovery-port 12000
```

#### Testnet Setup
```bash
anchor node \
  --network holesky \
  --data-dir ./testnet-data \
  --eth1-endpoint http://localhost:8545 \
  --beacon-node-endpoint http://localhost:5052 \
  --debug-level debug
```

## Production Deployment Examples

### Systemd Service Configuration
```ini
[Unit]
Description=Anchor SSV Node
After=network.target
Wants=network.target

[Service]
Type=simple
User=anchor
Group=anchor
Restart=always
RestartSec=5
ExecStart=/usr/local/bin/anchor node \
  --data-dir /var/lib/anchor \
  --logfile-dir /var/log/anchor \
  --logfile-max-size 500MB \
  --network mainnet
Environment=RUST_LOG=info
Environment=RUST_BACKTRACE=1

[Install]
WantedBy=multi-user.target
```

### Docker Deployment
```bash
# Run anchor in Docker container
docker run -d \
  --name anchor-node \
  --restart unless-stopped \
  -v /host/data:/data \
  -v /host/logs:/logs \
  -p 13000:13000 \
  -p 12000:12000/udp \
  anchor:latest node \
  --data-dir /data \
  --logfile-dir /logs \
  --network mainnet
```

### Kubernetes Deployment
```yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: anchor-node
spec:
  replicas: 1
  selector:
    matchLabels:
      app: anchor-node
  template:
    metadata:
      labels:
        app: anchor-node
    spec:
      containers:
      - name: anchor
        image: anchor:latest
        args:
          - node
          - --data-dir=/data
          - --logfile-dir=/logs
          - --network=mainnet
        ports:
        - containerPort: 13000
        - containerPort: 12000
          protocol: UDP
        volumeMounts:
        - name: data
          mountPath: /data
        - name: logs
          mountPath: /logs
        env:
        - name: RUST_LOG
          value: "info"
        - name: RUST_BACKTRACE
          value: "1"
      volumes:
      - name: data
        persistentVolumeClaim:
          claimName: anchor-data
      - name: logs
        persistentVolumeClaim:
          claimName: anchor-logs
```

## Monitoring and Debugging

### Health Checks
```bash
# Check if node is running (returns when node responds)
curl http://localhost:15000/health

# Monitor logs in real-time
tail -f ./logs/anchor.log

# Check specific component logs
grep "processor" ./logs/anchor.log | tail -20
```

### Performance Monitoring
```bash
# Start with metrics enabled
anchor node \
  --metrics \
  --metrics-address 0.0.0.0 \
  --metrics-port 8080

# Monitor resource usage
htop -p $(pgrep anchor)

# Network connectivity check
netstat -tlnp | grep anchor
```

### Debugging Common Issues

#### Connection Problems
```bash
# Debug network connectivity
anchor node --debug-level debug 2>&1 | grep -i "network\|connection"

# Check peer discovery
anchor node --debug-level trace 2>&1 | grep -i "discv5\|peer"
```

#### Key-related Issues
```bash
# Validate keystore format
anchor keygen --validate --keystore ./validator.json

# Test key splitting
anchor keysplit \
  --keystore ./test-key.json \
  --password ./password.txt \
  --operators 3 \
  --threshold 2 \
  --output-dir ./test-split \
  --dry-run
```

## Integration Examples

### With Ethereum Clients

#### Geth Integration
```bash
# Start with Geth
anchor node \
  --eth1-endpoint http://localhost:8545 \
  --eth1-timeout 30s \
  --network mainnet
```

#### Lighthouse Integration
```bash
# Connect to Lighthouse beacon node
anchor node \
  --beacon-node-endpoint http://localhost:5052 \
  --beacon-timeout 10s \
  --network mainnet
```

### Configuration File Usage
```yaml
# config.yaml
network: mainnet
data_dir: /var/lib/anchor
debug_level: info
eth1_endpoint: http://geth:8545
beacon_node_endpoint: http://lighthouse:5052
p2p_port: 13000
discovery_port: 12000
logfile_dir: /var/log/anchor
logfile_max_size: 200MB

# Usage:
anchor --config ./config.yaml node
```

## Troubleshooting Examples

### Common Error Scenarios

#### Data Directory Issues
```bash
# Create data directory with proper permissions
sudo mkdir -p /var/lib/anchor
sudo chown anchor:anchor /var/lib/anchor
sudo chmod 750 /var/lib/anchor

# Run with explicit data directory
anchor --data-dir /var/lib/anchor node
```

#### Port Conflicts
```bash
# Check port availability
netstat -tlnp | grep :13000

# Use alternative ports
anchor node \
  --p2p-port 13001 \
  --discovery-port 12001 \
  --metrics-port 8081
```

#### Memory Issues
```bash
# Run with limited memory
systemd-run --scope -p MemoryMax=2G anchor node

# Monitor memory usage
watch 'ps aux | grep anchor'
```

These examples cover the most common usage patterns and deployment scenarios for the Anchor binary in SSV network operations.