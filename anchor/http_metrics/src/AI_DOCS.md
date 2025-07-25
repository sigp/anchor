# HTTP Metrics Component - AI Documentation

## Overview

The `http_metrics` component provides a Prometheus-compatible metrics server for the Anchor SSV (Secret Shared Validator) client. It serves as a monitoring and observability layer, exposing critical validator performance and network health metrics through a standard HTTP/REST interface.

## Purpose & Role

This component serves as the primary telemetry endpoint for Anchor validators, enabling operators to:
- Monitor validator duty performance (attestations, proposals)
- Track network connectivity and peer status
- Observe system health and timing metrics
- Integrate with standard monitoring stacks (Prometheus, Grafana)

## Architecture

### Core Components

```
┌─────────────────┐    ┌──────────────────┐    ┌─────────────────┐
│   Main Client   │───▶│  Shared State    │◀───│ Metrics Server  │
│                 │    │                  │    │                 │
│ - Genesis Time  │    │ - Genesis Time   │    │ - HTTP Handler  │
│ - Duties Svc    │    │ - Duties Service │    │ - Prometheus    │
│ - Network Reg   │    │ - Network Reg    │    │ - CORS Support  │
└─────────────────┘    └──────────────────┘    └─────────────────┘
                                                         │
                                                         ▼
                                               ┌─────────────────┐
                                               │  /metrics       │
                                               │  Endpoint       │
                                               └─────────────────┘
```

### Key Structures

1. **`Shared<E: EthSpec>`**: Thread-safe container for shared state
   - `genesis_time`: Blockchain genesis timestamp
   - `duties_service`: Validator duties tracking service
   - `network_registry`: libp2p network metrics registry

2. **`Config`**: Server configuration
   - `enabled`: Feature toggle
   - `listen_addr`/`listen_port`: Binding configuration
   - `allow_origin`: CORS configuration

## Data Flow

1. **Initialization**: Main client creates shared state and spawns metrics server
2. **Population**: Client continuously updates shared state with live data:
   - Genesis time (once known)
   - Duties service reference
   - Network metrics registry
3. **Serving**: HTTP server reads shared state and formats as Prometheus metrics
4. **Graceful Shutdown**: Server shuts down with main client

## Integration Points

### CLI Integration
- `--metrics`: Enable metrics server
- `--metrics-address`: Bind address (default: 127.0.0.1)
- `--metrics-port`: Bind port (default: 5164)

### Client Integration
Located in `client/src/lib.rs:147-168`:
- Conditional server startup based on config
- Shared state creation and population
- Background task spawning with shutdown coordination

### Metrics Categories

1. **Validator Metrics**:
   - Genesis distance calculations
   - Proposer duty counts (current/next epoch)
   - Attester duty counts (current/next epoch)

2. **Network Metrics**:
   - libp2p connection status
   - Peer discovery metrics
   - Network health indicators

3. **System Metrics**:
   - Health check results
   - Discovery service status

## Threading Model

- **Main Thread**: Populates shared state with live data
- **Metrics Thread**: Serves HTTP requests, reads shared state
- **Synchronization**: `Arc<RwLock<Shared<E>>>` for thread-safe access

## Error Handling

- Prometheus encoding errors return HTTP 500
- Network binding failures logged and handled gracefully
- Missing data (genesis time, services) handled with conditional checks

## Performance Considerations

- Read-heavy workload on shared state (metrics serving)
- Write-light workload (periodic state updates)
- RwLock optimized for concurrent reads
- Lazy metric collection on request (not continuous)

## Dependencies

**Key External Dependencies**:
- `axum`: HTTP server framework
- `prometheus_client`: Metrics encoding
- `tower-http`: CORS middleware
- `lighthouse_network`: Network metrics integration
- `validator_metrics`: Validator-specific metrics

**Internal Dependencies**:
- `anchor_validator_store`: Validator data access
- `duties_service`: Duty tracking
- `slot_clock`: Time/slot calculations

## Future Considerations

The documentation notes this may be temporary until the Lighthouse VC moves to axum, suggesting potential consolidation opportunities. The component is designed to be easily replaceable or mergeable with upstream lighthouse metrics infrastructure.