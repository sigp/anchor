# Anchor Client Component Specification

## Technical Architecture

### Core Components

#### Client (`lib.rs`)
The main `Client` struct serves as the application entry point and orchestrator:

**Key Responsibilities:**
- System initialization and configuration processing
- Service lifecycle management
- Coordinator for all subsystems (networking, consensus, validator services)

**Main Flow (`Client::run`):**
1. **System resource optimization** (file descriptor limits via `fdlimit::raise_fd_limit()`)
2. **Network specification and genesis validation** (rejects mainnet, requires testnet)
3. **Operator key derivation** and RSA public key generation
4. **Processor initialization** (`processor::spawn()`)
5. **HTTP services setup** (metrics server on configurable port, HTTP API server)
6. **Database initialization** (SQLite at `data_dir/anchor_db.sqlite`, supports impostor mode)
7. **Slashing protection setup** (`slashing_protection.sqlite`)
8. **Beacon node connection** establishment with fallback support and health monitoring
9. **Genesis synchronization** and slot clock initialization
10. **SSV event syncer** for Ethereum execution layer monitoring
11. **P2P network initialization** with subnet service and message routing
12. **Validator services orchestration**:
    - Duties service with selection proof configuration
    - Block service with optional proposer nodes
    - Attestation service
    - Preparation service (validator registration)
    - Sync committee service
    - Metadata service

#### Configuration System (`config.rs`)
Centralized configuration management with CLI integration:

**Configuration Structure:**
```rust
pub struct Config {
    pub global_config: GlobalConfig,
    pub beacon_nodes: Vec<SensitiveUrl>,
    pub execution_nodes: Vec<SensitiveUrl>,
    pub network: network::Config,
    pub http_api: http_api::Config,
    pub http_metrics: http_metrics::Config,
    // ... additional configuration fields
}
```

**Key Features:**
- CLI argument parsing and validation
- Network address configuration (IPv4/IPv6 dual-stack support)
- TLS certificate management for secure connections
- Service-specific configuration (HTTP API, metrics, networking)

#### Key Management (`key.rs`)
Secure operator key handling with multiple storage formats:

**Function Signature:**
```rust
pub(crate) fn read_or_generate_private_key(
    data_dir: &Path,
    key_file: Option<&Path>,
    password_file: Option<&Path>,
) -> Result<Rsa<Private>, String>
```

**Supported Key Formats:**
- **Unencrypted keys** (`.txt` files) - Base64 encoded RSA-2048 keys
- **Encrypted keys** (`.json` files) - Password-protected RSA keys with JSON serialization

**Key Resolution Priority:**
1. Explicitly provided `key_file` path (must exist or fail)
2. `data_dir/unencrypted_private_key.txt`
3. `data_dir/encrypted_private_key.json`
4. Generate new 2048-bit RSA key if none found

**Key Operations:**
- Automatic 2048-bit RSA key generation using OpenSSL
- Password-based encryption/decryption with zeroized memory handling
- Public key derivation and export to `data_dir/public_key.txt`
- Secure key storage with atomic file operations (`File::create_new`)

#### CLI Interface (`cli.rs`)
Comprehensive command-line interface using `clap`:

**Major Configuration Categories:**
- **Network Configuration**: Listen addresses, ports, ENR settings
- **External Services**: Beacon nodes, execution endpoints
- **API Configuration**: HTTP API, metrics endpoints
- **Performance Tuning**: Worker threads, queue sizes
- **Security Options**: TLS certificates, slashing protection
- **MEV Integration**: Builder proposals, boost factors

#### Notification System (`notifier.rs`)
Periodic status reporting and metrics collection:

**Function Signature:**
```rust
pub fn spawn_notifier<E: EthSpec, T: SlotClock + 'static>(
    duties_service: Arc<DutiesService<AnchorValidatorStore<T, E>, T>>,
    network_state: watch::Receiver<NetworkState>,
    synced: watch::Receiver<bool>,
    executor: TaskExecutor,
    spec: &ChainSpec,
)
```

**Notification Schedule:**
- Executes once per slot, at the halfway point (slot_duration / 2)
- Uses SlotClock for precise timing alignment with Ethereum consensus

**Status Categories:**
- **"Syncing"**: Node is syncing, no operator ID found
- **"Synced, waiting for operator key"**: Synced but operator not registered on-chain
- **"Operator present, waiting for sync"**: Operator registered but not synced
- **"Operator active"**: Synced operator with active validators
- **"Operator ready, no validators"**: Ready operator with no validator assignments

**Metrics Integration:**
- Integrates with `validator_services::notifier_service` for detailed validator metrics
- Reports operator ID, cluster count, and validator assignments

## Data Flow Architecture

### Service Initialization Sequence
1. **Configuration Processing**: CLI args → Config struct
2. **Key Management**: Load/generate operator keys
3. **Database Setup**: Network state and slashing protection
4. **Network Layer**: P2P networking and discovery
5. **Consensus Layer**: Beacon node connections and sync
6. **Validator Services**: Duties, attestations, proposals
7. **Monitoring**: HTTP APIs and metrics collection

### Message Flow
1. **Inbound**: P2P network → Message validation → Service routing
2. **Outbound**: Service decisions → Message creation → P2P broadcast
3. **Beacon Chain**: Duty retrieval → Local processing → Submission

## Technical Specifications

### Network Protocol Support
- **Discovery**: ENR-based peer discovery
- **Transport**: TCP, UDP, QUIC protocols
- **Addressing**: IPv4/IPv6 dual-stack support
- **Ports**: Configurable with smart defaults (12001 discovery, 13001 TCP)

### Cryptographic Operations
- **RSA Key Management**: 2048-bit keys for operator identity
- **Secret Sharing**: Integration with SSV signature schemes
- **Slashing Protection**: Database-backed protection against invalid signatures

### Database Layer
- **Network Database**: SQLite-based operator and cluster state
- **Slashing Protection**: Separate SQLite database for validator safety

### HTTP Endpoints
- **API Server**: RESTful interface for external integrations
- **Metrics**: Prometheus-compatible metrics endpoint
- **CORS Support**: Configurable cross-origin resource sharing

### Performance Characteristics
- **Concurrent Processing**: Tokio-based async runtime with `TaskExecutor`
- **Worker Management**: Configurable thread pool sizing via `--max-workers`
- **Queue Management**: Per-service queue size tuning via `--work-queue-size`
- **Timeout Handling**: Adaptive timeouts based on beacon node availability:
  - Fast timeouts when fallback nodes available (quotient-based)
  - Full slot duration timeouts for single nodes or `--use-long-timeouts`
  - Specific quotients for different operations (attestation: 1/4, proposal: 1/2, etc.)

### Timeout Configuration Constants
```rust
const HTTP_ATTESTATION_TIMEOUT_QUOTIENT: u32 = 4;        // slot_duration / 4
const HTTP_PROPOSAL_TIMEOUT_QUOTIENT: u32 = 2;          // slot_duration / 2  
const HTTP_ATTESTER_DUTIES_TIMEOUT_QUOTIENT: u32 = 4;   // slot_duration / 4
const HTTP_SYNC_DUTIES_TIMEOUT_QUOTIENT: u32 = 4;       // slot_duration / 4
```

### Security Features
- **TLS Support**: Custom certificate validation for external connections
- **Key Protection**: Encrypted key storage with password protection
- **Slashing Prevention**: Comprehensive slashing protection database
- **Network Security**: Peer scoring and topic-based filtering

## Error Handling

### Critical Error Conditions
- **Genesis Validation**: Mainnet rejection (testnet only)
- **Key Management**: Missing or corrupted operator keys
- **Network Failures**: Beacon node connectivity issues
- **Database Corruption**: SQLite database access failures

### Recovery Mechanisms
- **Beacon Node Fallback**: Automatic failover between configured nodes
- **Service Restart**: Individual service failure isolation
- **Database Recovery**: Automatic database repair and migration

## Dependencies

### Core Dependencies
- **Tokio**: Async runtime and networking
- **OpenSSL**: Cryptographic operations
- **SQLite**: Database storage
- **CLAP**: Command-line argument parsing
- **Tracing**: Structured logging

### Ethereum Integration
- **eth2**: Beacon chain API client
- **types**: Ethereum consensus types
- **slot_clock**: Consensus time management

### Networking
- **libp2p**: P2P networking stack
- **multiaddr**: Network address formatting
- **ENR**: Ethereum Node Records