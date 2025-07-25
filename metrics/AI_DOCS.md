# Anchor Metrics Component - AI Documentation

## Overview
The Anchor Metrics component is a Docker-based monitoring and observability stack that provides real-time metrics collection, storage, and visualization for Anchor nodes. It consists of a Prometheus server for metrics scraping and a Grafana dashboard for web-based visualization.

## Architecture
This component implements a containerized monitoring solution with the following key elements:

### Core Components
- **Prometheus Server**: Time-series database that scrapes metrics from Anchor nodes
- **Grafana Dashboard**: Web-based visualization and alerting platform
- **Service Discovery**: File-based target discovery for dynamic node monitoring

### Container Architecture
- Uses Docker Compose for orchestration
- Network mode: host (for direct access to local services)
- Persistent volumes for data retention
- Custom Docker images with tailored configurations

## Technical Implementation

### Prometheus Configuration
- Global scrape interval: 15s
- Local Anchor job scrape interval: 5s
- File-based service discovery from `/prometheus/targets/scrape-targets.json`
- External labels for monitoring identification

### Grafana Setup
- Default credentials: admin/changeme
- Persistent data storage via Docker volumes
- Custom configuration via `grafana.ini`
- Dashboard import capability for JSON files

### Service Discovery
The metrics stack uses file-based service discovery to dynamically monitor Anchor nodes:
- Target configuration in `scrape-targets.json`
- Default monitoring port: 5154
- Support for multiple node instances
- Optional Lighthouse node integration

## Data Flow
1. Anchor nodes expose metrics on HTTP endpoints (default: port 5154)
2. Prometheus scrapes these endpoints based on file-based service discovery
3. Metrics are stored in Prometheus time-series database
4. Grafana queries Prometheus for dashboard visualization
5. Users access dashboards via web interface on port 3000

## Monitoring Capabilities
- Real-time node performance metrics
- Historical data analysis
- Custom dashboard creation and import
- Alert configuration (through Grafana)
- Multi-node monitoring support

## Security Considerations
- Default binding to localhost (127.0.0.1) for security
- Configurable public hosting via `grafana.ini` modifications
- Password-protected Grafana access
- Network isolation via Docker containers

## Deployment Model
- Requires Docker and Docker Compose
- Horizontal scaling through multiple scrape targets
- Persistent storage for historical data
- Environment-specific configuration through file mounting