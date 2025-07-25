# Anchor Metrics Component Specification

## Service Specifications

### Prometheus Service
**Image**: Custom build from `prometheus/Dockerfile`
**Network**: Host mode
**Restart Policy**: Always

#### Volume Mounts
- `prometheus-data:/prometheus` - Persistent metrics storage
- `./scrape-targets:/prometheus/targets` - Service discovery configuration

#### Configuration Files
- `prometheus.yml` - Main Prometheus configuration
- Global scrape interval: 15s
- Monitor label: 'anchor-docker'

### Grafana Service
**Image**: Custom build from `grafana/Dockerfile`
**Network**: Host mode  
**Restart Policy**: Always

#### Volume Mounts
- `grafana-data:/var/lib/grafana` - Persistent dashboard and user data

#### Configuration Files
- `grafana.ini` - Grafana server configuration
- Provisioning configs in `provisioning/` directory

## API Interfaces

### Prometheus HTTP API
**Base URL**: `http://localhost:9090`

#### Endpoints
- `/targets` - View scrape target status
- `/api/v1/query` - Instant queries
- `/api/v1/query_range` - Range queries
- `/api/v1/label/{label_name}/values` - Label values
- `/metrics` - Prometheus own metrics

### Grafana HTTP API
**Base URL**: `http://localhost:3000`
**Default Credentials**: admin/changeme

#### Dashboard API
- `GET /api/dashboards/home` - Home dashboard
- `POST /api/dashboards/db` - Create/update dashboard
- `GET /api/dashboards/uid/{uid}` - Get dashboard by UID
- `DELETE /api/dashboards/uid/{uid}` - Delete dashboard

#### Data Source API
- `GET /api/datasources` - List data sources
- `POST /api/datasources` - Create data source
- `GET /api/datasources/{id}` - Get data source by ID

## Configuration Schemas

### Scrape Targets Configuration
**File**: `scrape-targets/scrape-targets.json`

```json
[
  {
    "labels": {
      "job": "string"
    },
    "targets": [
      "host:port"
    ]
  }
]
```

#### Schema Fields
- `labels.job` (string, required): Job identifier for grouping targets
- `targets` (array, required): List of host:port combinations to scrape

### Prometheus Configuration
**File**: `prometheus/prometheus.yml`

#### Global Configuration
- `scrape_interval`: Default scrape frequency (duration)
- `external_labels`: Labels added to all metrics

#### Scrape Configuration
- `job_name`: Identifier for the scrape job
- `scrape_interval`: Job-specific scrape frequency
- `file_sd_configs`: File-based service discovery settings

### Docker Compose Schema
**File**: `docker-compose.yaml`

#### Service Definition
- `build.context`: Build context directory
- `volumes`: Volume mount specifications
- `restart`: Container restart policy
- `network_mode`: Network configuration mode

## Port Specifications

### Default Ports
- **Grafana Web UI**: 3000
- **Prometheus Web UI**: 9090
- **Anchor Node Metrics**: 5154
- **Lighthouse Node Metrics**: 8080 (optional)

### Port Requirements
- All services use host networking mode
- Ports must be available on the host system
- No port mapping required due to host networking

## Data Formats

### Metrics Format
Prometheus text-based exposition format:
```
# HELP metric_name Description of the metric
# TYPE metric_name counter
metric_name{label1="value1",label2="value2"} value timestamp
```

### Dashboard Format
Grafana dashboard JSON schema with:
- Dashboard metadata (title, tags, time settings)
- Panel definitions (queries, visualizations, thresholds)
- Template variables for dynamic dashboards
- Alert rule configurations

## Security Specifications

### Access Control
- Grafana: Username/password authentication
- Prometheus: No authentication (localhost binding)
- Default binding: 127.0.0.1 (localhost only)

### Network Security
- Container-to-host communication
- No external network exposure by default
- Configurable public access via `grafana.ini`

### Data Protection
- Persistent volumes for data retention
- Container restart policies for availability
- Backup considerations for volume data