# Anchor Metrics Usage Examples

## Basic Setup and Usage

### 1. Starting Anchor with Metrics
```bash
# Start an Anchor node with metrics enabled
anchor --metrics

# Start with custom metrics port
anchor --metrics --metrics-port 5155

# Start with metrics and custom bind address
anchor --metrics --metrics-address 0.0.0.0:5154
```

### 2. Launch Metrics Stack
```bash
# Navigate to metrics directory
cd metrics

# Start the monitoring stack
docker-compose up --build

# Start in detached mode
docker-compose up --build -d

# View logs
docker-compose logs -f
```

### 3. Basic Health Checks
```bash
# Check if Prometheus can reach targets
curl http://localhost:9090/targets

# Check Prometheus metrics endpoint
curl http://localhost:9090/metrics

# Check Grafana health
curl http://localhost:3000/api/health
```

## Configuration Examples

### Adding Multiple Anchor Nodes
**File**: `scrape-targets/scrape-targets.json`
```json
[
  {
    "labels": {
      "job": "anchor-mainnet"
    },
    "targets": [
      "localhost:5154",
      "192.168.1.100:5154",
      "192.168.1.101:5154"
    ]
  },
  {
    "labels": {
      "job": "anchor-testnet"
    },
    "targets": [
      "localhost:5155"
    ]
  }
]
```

### Multi-Client Monitoring (Anchor + Lighthouse)
**File**: `scrape-targets/scrape-targets-lighthouse.json`
```json
[
  {
    "labels": {
      "job": "anchor-nodes"
    },
    "targets": [
      "localhost:5154"
    ]
  },
  {
    "labels": {
      "job": "lighthouse-beacon"
    },
    "targets": [
      "localhost:5054"
    ]
  },
  {
    "labels": {
      "job": "lighthouse-validator"
    },
    "targets": [
      "localhost:5064"
    ]
  }
]
```

### Custom Prometheus Configuration
**File**: `prometheus/prometheus.yml`
```yaml
global:
  scrape_interval: 10s
  evaluation_interval: 10s
  external_labels:
    monitor: 'anchor-production'
    datacenter: 'us-east-1'

scrape_configs:
  - job_name: 'anchor-nodes'
    scrape_interval: 5s
    scrape_timeout: 4s
    file_sd_configs:
      - files:
          - '/prometheus/targets/scrape-targets.json'
        refresh_interval: 30s
    
    relabel_configs:
      - source_labels: [__address__]
        target_label: __param_target
      - source_labels: [__param_target]
        target_label: instance
      - target_label: __address__
        replacement: localhost:5154

  - job_name: 'prometheus'
    static_configs:
      - targets: ['localhost:9090']
```

## Grafana Dashboard Examples

### Importing Existing Dashboards
1. **Via Web UI**:
   - Navigate to `http://localhost:3000`
   - Login with admin/changeme
   - Go to `Dashboards` → `Manage` → `Import`
   - Upload `.json` file from `dashboards/` directory

2. **Via API**:
```bash
# Import dashboard via API
curl -X POST \
  http://admin:changeme@localhost:3000/api/dashboards/db \
  -H 'Content-Type: application/json' \
  -d @dashboards/Summary.json
```

### Creating Custom Panels
**Example**: Anchor Node Count Panel
```json
{
  "title": "Active Anchor Nodes",
  "type": "stat",
  "targets": [
    {
      "expr": "count(up{job=\"anchor-nodes\"} == 1)",
      "legendFormat": "Active Nodes"
    }
  ],
  "fieldConfig": {
    "defaults": {
      "color": {
        "mode": "thresholds"
      },
      "thresholds": {
        "steps": [
          {"color": "red", "value": 0},
          {"color": "yellow", "value": 1},
          {"color": "green", "value": 2}
        ]
      }
    }
  }
}
```

## Query Examples

### Prometheus Queries (PromQL)

#### Basic Node Health
```promql
# Check if nodes are up
up{job="anchor-nodes"}

# Count of active nodes
count(up{job="anchor-nodes"} == 1)

# Nodes that have been down for more than 5 minutes
up{job="anchor-nodes"} == 0
```

#### Performance Metrics
```promql
# Memory usage over time
anchor_memory_usage_bytes{job="anchor-nodes"}

# CPU usage rate
rate(anchor_cpu_seconds_total{job="anchor-nodes"}[5m])

# Network I/O rate
rate(anchor_network_bytes_total{job="anchor-nodes"}[5m])
```

#### Anchor-Specific Metrics
```promql
# Validator performance
anchor_validator_duties_total{job="anchor-nodes"}

# Attestation success rate
rate(anchor_attestations_successful_total{job="anchor-nodes"}[5m]) / 
rate(anchor_attestations_total{job="anchor-nodes"}[5m])

# Peer connections
anchor_network_peers{job="anchor-nodes"}
```

## Advanced Usage

### Custom Alert Rules
**File**: `prometheus/alert-rules.yml`
```yaml
groups:
  - name: anchor-alerts
    rules:
      - alert: AnchorNodeDown
        expr: up{job="anchor-nodes"} == 0
        for: 2m
        labels:
          severity: critical
        annotations:
          summary: "Anchor node {{ $labels.instance }} is down"
          description: "Anchor node has been down for more than 2 minutes"

      - alert: HighMemoryUsage
        expr: anchor_memory_usage_bytes{job="anchor-nodes"} > 8e9
        for: 5m
        labels:
          severity: warning
        annotations:
          summary: "High memory usage on {{ $labels.instance }}"
```

### Docker Compose Overrides
**File**: `docker-compose.override.yml`
```yaml
version: "3.3"
services:
  prometheus:
    ports:
      - "9090:9090"
    environment:
      - PROMETHEUS_RETENTION_TIME=30d
      - PROMETHEUS_RETENTION_SIZE=50GB
  
  grafana:
    ports:
      - "3000:3000"
    environment:
      - GF_SECURITY_ADMIN_PASSWORD=custom-password
      - GF_INSTALL_PLUGINS=grafana-piechart-panel
```

### Backup and Restore
```bash
# Backup Grafana data
docker-compose exec grafana tar -czf /tmp/grafana-backup.tar.gz /var/lib/grafana
docker cp $(docker-compose ps -q grafana):/tmp/grafana-backup.tar.gz ./grafana-backup.tar.gz

# Backup Prometheus data
docker-compose exec prometheus tar -czf /tmp/prometheus-backup.tar.gz /prometheus
docker cp $(docker-compose ps -q prometheus):/tmp/prometheus-backup.tar.gz ./prometheus-backup.tar.gz

# Restore from backup
docker-compose down
docker volume rm metrics_grafana-data metrics_prometheus-data
docker-compose up -d
# Copy backup files back to containers and extract
```

## Troubleshooting Examples

### Common Issues and Solutions

#### Prometheus Can't Reach Targets
```bash
# Check if Anchor node is exposing metrics
curl http://localhost:5154/metrics

# Verify scrape targets configuration
cat scrape-targets/scrape-targets.json

# Check Prometheus logs
docker-compose logs prometheus
```

#### Grafana Dashboard Issues
```bash
# Reset Grafana admin password
docker-compose exec grafana grafana-cli admin reset-admin-password newpassword

# Check Grafana logs
docker-compose logs grafana

# Verify data source connectivity
curl -u admin:changeme http://localhost:3000/api/datasources
```

#### Performance Optimization
```bash
# Reduce scrape interval for high-load scenarios
# Edit prometheus.yml:
# scrape_interval: 30s  # Instead of 15s

# Limit metric retention
# Add to prometheus.yml global section:
# retention: 15d
# retention_size: 10GB
```