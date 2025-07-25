# Logging Component Specification

## Module Structure

### `lib.rs`
**Exports**: Public API surface
```rust
pub use count_layer::CountLayer;
pub use logging::*;
pub mod utils;
```

### `logging.rs`
**Primary Functions**:
- `init_file_logging(logs_dir: &Path, config: FileLoggingFlags) -> Option<LoggingLayer>`

**Key Types**:
```rust
pub struct FileLoggingFlags {
    pub logfile_debug_level: Level,      // Default: DEBUG
    pub logfile_max_size: u64,           // Default: 50MB
    pub logfile_max_number: u64,         // Default: 100 files
    pub logfile_dir: Option<PathBuf>,    // Optional directory
    pub logfile_compression: bool,       // Gzip compression
    pub logfile_color: bool,             // Color output
}

pub struct LoggingLayer {
    pub non_blocking_writer: NonBlocking,
    pub guard: WorkerGuard,
}
```

**Behavior**:
- Returns `None` if file logging is disabled (max_size or max_number = 0)
- Creates rolling file appender with configurable rotation
- Uses non-blocking I/O to prevent application blocking

### `count_layer.rs`
**Metrics Exposed**:
```rust
// Global counters
static INFOS_TOTAL: LazyLock<metrics::Result<metrics::IntCounter>>
static WARNS_TOTAL: LazyLock<metrics::Result<metrics::IntCounter>>  
static ERRORS_TOTAL: LazyLock<metrics::Result<metrics::IntCounter>>

// Per-dependency counters
static DEP_INFOS_TOTAL: LazyLock<metrics::Result<metrics::IntCounterVec>>
static DEP_WARNS_TOTAL: LazyLock<metrics::Result<metrics::IntCounterVec>>
static DEP_ERRORS_TOTAL: LazyLock<metrics::Result<metrics::IntCounterVec>>
```

**Implementation**:
```rust
impl<S: tracing_core::Subscriber> tracing_subscriber::layer::Layer<S> for CountLayer
```

**Event Processing**:
- Ignores span events, only processes log events
- Increments global counters for all INFO/WARN/ERROR events
- Extracts crate name from target for dependency-specific metrics
- Uses normalized metadata when available

### `tracing_libp2p_discv5_layer.rs`
**Primary Function**:
```rust
pub fn create_libp2p_discv5_tracing_layer(
    base_tracing_log_path: Option<PathBuf>,
    max_log_size: u64,
) -> Option<Libp2pDiscv5TracingLayer>
```

**Target Filtering**:
- `libp2p_gossipsub` → writes to `libp2p.log`
- `discv5` → writes to `discv5.log`
- Other targets are ignored

**Log Format**: `{timestamp} {level} {message}\n`
- Timestamp format: `%Y-%m-%d %H:%M:%S`

**File Management**:
- Size-based rotation with 1 backup file
- Separate files for different network protocols

### `utils.rs`
**Primary Function**:
```rust
pub fn build_workspace_filter() -> Result<FilterFn<impl Fn(&tracing::Metadata) -> bool + Clone>, String>
```

**Filtering Logic**:
- Includes workspace crates (from `workspace_crates!()` macro)
- Includes lighthouse crates: `beacon_node_fallback`, `slashing_protection`, `task_executor`, `validator_services`
- Filters by first segment of target (crate name)

## Configuration Interface

### CLI Arguments (via `FileLoggingFlags`)
- `--logfile-debug-level`: Log level for file output (default: DEBUG)
- `--logfile-max-size`: Max size per file in MB (default: 50, 0 disables)
- `--logfile-max-number`: Max number of files (default: 100, 0 disables)
- `--logfile-dir`: Directory for log files
- `--logfile-compression`: Enable gzip compression
- `--logfile-color`: Enable colored output in files

## Error Handling

### File Logging
- Returns `None` on appender creation failure
- Prints error to stderr: `"Failed to create rolling file appender: {error}"`

### Network Tracing
- Exits process on failure: `std::process::exit(1)`
- Error messages for libp2p/discv5 appender failures

### Metrics
- Uses `Result<T>` for metric creation
- Graceful degradation if metrics fail to initialize

## Thread Safety
- Uses `LazyLock` for static initialization
- Non-blocking writers prevent I/O blocking
- Worker guards ensure proper cleanup

## Dependencies
- `tracing-core ^0.1`: Core tracing infrastructure
- `tracing-subscriber`: Layer implementation
- `logroller ^0.1.8`: Rolling file appender
- `metrics`: Counter implementation
- `chrono ^0.4`: Timestamp formatting
- `clap`: CLI parsing (workspace dependency)