# Anchor Client Usage Examples

## Basic Client Startup

### Minimal Configuration
```bash
# Start with default settings (localhost beacon node and execution client)
anchor --data-dir /path/to/data

# Specify custom beacon node
anchor --data-dir /path/to/data --beacon-nodes http://beacon.example.com:5052

# Multiple beacon nodes for fallback
anchor --data-dir /path/to/data \
  --beacon-nodes http://primary.beacon.com:5052,http://backup.beacon.com:5052
```

### Network Configuration
```bash
# Configure listening addresses and ports
anchor --data-dir /path/to/data \
  --listen-addresses 0.0.0.0 \
  --port 12001 \
  --discovery-port 12001

# IPv6 configuration
anchor --data-dir /path/to/data \
  --listen-addresses :: \
  --port6 12001

# Dual-stack IPv4/IPv6
anchor --data-dir /path/to/data \
  --listen-addresses 0.0.0.0 --listen-addresses :: \
  --port 12001 --port6 12001
```

## Key Management Examples

### Using Existing Keys
```bash
# Unencrypted key file
anchor --data-dir /path/to/data \
  --key-file /path/to/unencrypted_key.txt

# Encrypted key with password file
anchor --data-dir /path/to/data \
  --key-file /path/to/encrypted_key.json \
  --password-file /path/to/password.txt

# Encrypted key with interactive password prompt
anchor --data-dir /path/to/data \
  --key-file /path/to/encrypted_key.json
```

### Key Generation Scenarios
```bash
# First run - generates new unencrypted key automatically
anchor --data-dir /new/data/directory

# Generate encrypted key with password file
anchor --data-dir /new/data/directory \
  --password-file /path/to/password.txt
```

## API and Monitoring Configuration

### HTTP API Setup
```bash
# Enable HTTP API on default port (5062)
anchor --data-dir /path/to/data --http

# Custom HTTP configuration
anchor --data-dir /path/to/data \
  --http \
  --http-address 127.0.0.1 \
  --http-port 8080 \
  --unencrypted-http-transport

# Enable CORS for web applications
anchor --data-dir /path/to/data \
  --http \
  --http-allow-origin "https://dashboard.example.com"
```

### Metrics Configuration
```bash
# Enable Prometheus metrics
anchor --data-dir /path/to/data \
  --metrics \
  --metrics-address 127.0.0.1 \
  --metrics-port 9090

# High validator count metrics
anchor --data-dir /path/to/data \
  --metrics \
  --enable-high-validator-count-metrics
```

## Network and Discovery Configuration

### Boot Nodes and Discovery
```bash
# Custom boot nodes using ENRs
anchor --data-dir /path/to/data \
  --boot-nodes "enr:-..."

# Multiple boot nodes
anchor --data-dir /path/to/data \
  --boot-nodes "enr:-...","/ip4/192.168.1.1/tcp/12001/p2p/16Uiu2HAm..."

# Custom ENR settings for external connectivity
anchor --data-dir /path/to/data \
  --enr-address 203.0.113.1 \
  --enr-tcp-port 12001 \
  --enr-udp-port 12001
```

### Subnet and Scoring Configuration
```bash
# Subscribe to all subnets (for testing or high-connectivity nodes)
anchor --data-dir /path/to/data \
  --subscribe-all-subnets

# Disable gossipsub peer scoring
anchor --data-dir /path/to/data \
  --disable-gossipsub-peer-scoring
```

## MEV and Block Building Configuration

### Builder Integration
```bash
# Enable builder proposals
anchor --data-dir /path/to/data \
  --builder-proposals \
  --gas-limit 30000000

# Configure builder boost factor (prefer builder when 10% more valuable)
anchor --data-dir /path/to/data \
  --builder-proposals \
  --builder-boost-factor 110

# Always prefer builder proposals regardless of value
anchor --data-dir /path/to/data \
  --builder-proposals \
  --prefer-builder-proposals
```

## Performance Tuning Examples

### Worker and Queue Configuration
```bash
# Configure worker threads and queue sizes
anchor --data-dir /path/to/data \
  --max-workers 8 \
  --work-queue-size "attestation=1000,sync_contribution=500"
```

### Timeout Configuration
```bash
# Use longer timeouts (useful for slow networks)
anchor --data-dir /path/to/data \
  --use-long-timeouts
```

## Security Configuration

### TLS Certificate Management
```bash
# Custom TLS certificates for beacon node connections
anchor --data-dir /path/to/data \
  --beacon-nodes-tls-certs /path/to/beacon-cert.pem

# Multiple certificates
anchor --data-dir /path/to/data \
  --beacon-nodes-tls-certs /path/to/cert1.pem,/path/to/cert2.pem \
  --execution-nodes-tls-certs /path/to/execution-cert.pem
```

### Slashing Protection
```bash
# Disable slashing protection (DANGEROUS - only for testing)
anchor --data-dir /path/to/data \
  --disable-slashing-protection
```

## Advanced Configuration Examples

### Multi-Node Production Setup
```bash
# Production configuration with redundancy and monitoring
anchor --data-dir /opt/anchor/data \
  --beacon-nodes http://beacon1.internal:5052,http://beacon2.internal:5052 \
  --execution-rpc http://geth1.internal:8545,http://geth2.internal:8545 \
  --execution-ws ws://geth1.internal:8546 \
  --listen-addresses 0.0.0.0 \
  --port 12001 \
  --enr-address 203.0.113.10 \
  --http \
  --http-address 127.0.0.1 \
  --http-allow-origin "https://monitoring.example.com" \
  --metrics \
  --metrics-address 127.0.0.1 \
  --enable-high-validator-count-metrics \
  --builder-proposals \
  --builder-boost-factor 105 \
  --beacon-nodes-tls-certs /etc/ssl/certs/beacon-ca.pem
```

### Development and Testing Setup
```bash
# Development setup with debugging features
anchor --data-dir ./dev-data \
  --beacon-nodes http://localhost:5052 \
  --execution-rpc http://localhost:8545 \
  --execution-ws ws://localhost:8546 \
  --use-zero-ports \
  --http \
  --metrics \
  --subscribe-all-subnets \
  --disable-latency-measurement-service \
  --impostor 123  # Testing only - act as operator ID 123
```

### Docker and Container Usage
```bash
# Container setup with proper volume mounts
docker run -v /host/data:/data -p 12001:12001 -p 5062:5062 anchor \
  --data-dir /data \
  --listen-addresses 0.0.0.0 \
  --http \
  --http-address 0.0.0.0 \
  --unencrypted-http-transport \
  --beacon-nodes http://beacon-node.local:5052
```

## Configuration File Integration

### Using Global Configuration
The client integrates with global SSV network configurations that specify:
- Network parameters (chain specifications, domain types)
- Default boot nodes for the SSV network
- Network-specific settings

```bash
# Network configuration is typically loaded from:
# - Default embedded configurations for known networks
# - Custom configuration files specified via global config
anchor --data-dir /path/to/data --network testnet
```

**Note**: Mainnet is explicitly rejected by the client for safety. Only testnet configurations are supported.

## Logging and Debugging

### Structured Logging
```bash
# Basic logging configuration
anchor --data-dir /path/to/data \
  --log-level info

# File logging with rotation
anchor --data-dir /path/to/data \
  --log-level debug \
  --log-file /var/log/anchor/anchor.log \
  --log-max-size 100MB \
  --log-max-files 10
```

## Health Check Examples

### Service Status Verification
```bash
# Check if HTTP API is responding
curl http://localhost:5062/health

# Check Prometheus metrics (default port 5164)
curl http://localhost:5164/metrics

# Monitor specific metrics
curl -s http://localhost:5164/metrics | grep -E "(validator_|eth2_|anchor_)"

# Check sync status and operator information
curl http://localhost:5062/api/v1/status
```

## Troubleshooting Common Issues

### Key Management Issues
```bash
# Generate a new key if existing key is corrupted
rm /path/to/data/unencrypted_private_key.txt
rm /path/to/data/encrypted_private_key.json
anchor --data-dir /path/to/data  # Will generate new key

# Convert unencrypted key to encrypted
anchor keygen --data-dir /path/to/data --encrypt
```

### Network Connectivity Issues
```bash
# Test with zero ports to avoid port conflicts
anchor --data-dir /path/to/data --use-zero-ports

# Debug networking with all subnets enabled
anchor --data-dir /path/to/data \
  --subscribe-all-subnets \
  --log-level debug

# Check ENR configuration
anchor --data-dir /path/to/data \
  --enr-address $(curl -s ifconfig.me) \
  --enr-tcp-port 12001 \
  --enr-udp-port 12001
```

### Performance Optimization
```bash
# High-performance setup for many validators
anchor --data-dir /path/to/data \
  --max-workers $(nproc) \
  --work-queue-size "attestation=2000,sync_contribution=1000,proposal=500" \
  --enable-high-validator-count-metrics \
  --disable-latency-measurement-service
```

## Systemd Service Configuration

### Service File Example
```ini
[Unit]
Description=Anchor SSV Client
After=network-online.target
Wants=network-online.target

[Service]
Type=exec
User=anchor
Group=anchor
ExecStart=/usr/local/bin/anchor \
  --data-dir /var/lib/anchor \
  --beacon-nodes http://localhost:5052 \
  --execution-rpc http://localhost:8545 \
  --execution-ws ws://localhost:8546 \
  --http \
  --metrics \
  --log-level info
Restart=always
RestartSec=10
KillMode=mixed
TimeoutStopSec=30

[Install]
WantedBy=multi-user.target
```

### Service Management
```bash
# Install and start service
sudo systemctl daemon-reload
sudo systemctl enable anchor
sudo systemctl start anchor

# Monitor service logs
journalctl -u anchor -f

# Check service status
systemctl status anchor
```

These examples demonstrate the flexibility and comprehensive configuration options available in the Anchor Client, from basic setups to complex production deployments with full monitoring and redundancy.