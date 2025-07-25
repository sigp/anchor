# Logging Component Usage Examples

## Basic File Logging Setup

### Initialize File Logging
```rust
use logging::{init_file_logging, FileLoggingFlags};
use std::path::Path;
use tracing::Level;

// Configure file logging
let config = FileLoggingFlags {
    logfile_debug_level: Level::INFO,
    logfile_max_size: 100,        // 100MB per file
    logfile_max_number: 50,       // Keep 50 files
    logfile_dir: Some("/var/log/anchor".into()),
    logfile_compression: true,    // Enable gzip compression
    logfile_color: false,         // Disable colors in files
};

// Initialize file logging
let logs_dir = Path::new("/var/log/anchor");
if let Some(logging_layer) = init_file_logging(logs_dir, config) {
    // Use the logging layer with tracing subscriber
    // The guard must be kept alive for the duration of the program
    let _guard = logging_layer.guard;
}
```

### Disable File Logging
```rust
let config = FileLoggingFlags {
    logfile_debug_level: Level::INFO,
    logfile_max_size: 0,          // Disable by setting to 0
    logfile_max_number: 100,
    logfile_dir: None,
    logfile_compression: false,
    logfile_color: false,
};

// This will return None
let result = init_file_logging(Path::new("."), config);
assert!(result.is_none());
```

## Metrics Collection with CountLayer

### Basic CountLayer Setup
```rust
use logging::CountLayer;
use tracing_subscriber::{Registry, prelude::*};

// Create a tracing subscriber with the count layer
let subscriber = Registry::default()
    .with(CountLayer);

tracing::subscriber::set_global_default(subscriber)
    .expect("Failed to set tracing subscriber");

// Now all log events will be counted
tracing::info!("This will increment INFOS_TOTAL");
tracing::warn!("This will increment WARNS_TOTAL");
tracing::error!("This will increment ERRORS_TOTAL");
```

### Accessing Metrics
```rust
use logging::count_layer::{INFOS_TOTAL, WARNS_TOTAL, ERRORS_TOTAL};

// Read current counts
if let Ok(counter) = INFOS_TOTAL.as_ref() {
    let count = counter.get();
    println!("Total info logs: {}", count);
}

// Per-dependency metrics
use logging::count_layer::DEP_INFOS_TOTAL;
if let Ok(counter_vec) = DEP_INFOS_TOTAL.as_ref() {
    let count = counter_vec.get_metric_with_label_values(&["my_crate"]);
    // Use the counter...
}
```

## Network Protocol Tracing

### Setup libp2p and discv5 Tracing
```rust
use logging::create_libp2p_discv5_tracing_layer;
use std::path::PathBuf;
use tracing_subscriber::{Registry, prelude::*};

// Create the network tracing layer
let log_path = Some(PathBuf::from("/var/log/anchor/network"));
let max_size = 10; // 10MB per file
let network_layer = create_libp2p_discv5_tracing_layer(log_path, max_size);

if let Some(layer) = network_layer {
    // Add to subscriber
    let subscriber = Registry::default()
        .with(layer);
    
    tracing::subscriber::set_global_default(subscriber)
        .expect("Failed to set subscriber");
}

// These will be written to separate files:
// libp2p_gossipsub events → libp2p.log
// discv5 events → discv5.log
```

### Example Network Log Output
```
2024-01-15 14:30:25 INFO Received gossipsub message
2024-01-15 14:30:26 WARN discv5 peer timeout
2024-01-15 14:30:27 ERROR Failed to connect to peer
```

## Workspace Filtering

### Apply Workspace Filter
```rust
use logging::utils::build_workspace_filter;
use tracing_subscriber::{Registry, prelude::*};

// Create workspace filter
let workspace_filter = build_workspace_filter()
    .expect("Failed to build workspace filter");

// Apply filter to subscriber
let subscriber = Registry::default()
    .with(workspace_filter);

tracing::subscriber::set_global_default(subscriber)
    .expect("Failed to set subscriber");

// Only logs from workspace crates will be shown
```

## Complete Setup Example

### Full Logging Infrastructure
```rust
use logging::{
    init_file_logging, FileLoggingFlags, CountLayer,
    create_libp2p_discv5_tracing_layer,
    utils::build_workspace_filter,
};
use tracing::Level;
use tracing_subscriber::{Registry, prelude::*, fmt, EnvFilter};
use std::path::PathBuf;

fn setup_logging() -> Result<(), Box<dyn std::error::Error>> {
    // File logging configuration
    let file_config = FileLoggingFlags {
        logfile_debug_level: Level::DEBUG,
        logfile_max_size: 50,
        logfile_max_number: 100,
        logfile_dir: Some(PathBuf::from("/var/log/anchor")),
        logfile_compression: true,
        logfile_color: false,
    };

    // Initialize file logging
    let logs_dir = std::path::Path::new("/var/log/anchor");
    let file_layer = init_file_logging(logs_dir, file_config);

    // Network tracing
    let network_layer = create_libp2p_discv5_tracing_layer(
        Some(PathBuf::from("/var/log/anchor/network")),
        10, // 10MB
    );

    // Workspace filter
    let workspace_filter = build_workspace_filter()?;

    // Build subscriber with all layers
    let mut subscriber = Registry::default()
        .with(CountLayer)
        .with(workspace_filter)
        .with(fmt::layer().with_target(true))
        .with(EnvFilter::from_default_env());

    if let Some(file_layer) = file_layer {
        subscriber = subscriber.with(tracing_subscriber::fmt::layer()
            .with_writer(file_layer.non_blocking_writer)
            .with_ansi(false));
    }

    if let Some(network_layer) = network_layer {
        subscriber = subscriber.with(network_layer);
    }

    tracing::subscriber::set_global_default(subscriber)?;
    
    Ok(())
}
```

## CLI Usage Examples

### Command Line Arguments
```bash
# Enable file logging with custom settings
anchor --logfile-debug-level info \
       --logfile-max-size 100 \
       --logfile-max-number 50 \
       --logfile-dir /custom/log/path \
       --logfile-compression \
       --logfile-color

# Disable file logging
anchor --logfile-max-size 0

# Basic file logging with defaults
anchor --logfile-dir /var/log/anchor
```

### Configuration via Environment
```bash
# Set log level via environment
export RUST_LOG=debug
anchor --logfile-dir /var/log/anchor

# Combined with file-specific level
export RUST_LOG=info
anchor --logfile-debug-level debug --logfile-dir /var/log/anchor
```

## Testing Examples

### Unit Test with Logging
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tracing_test::traced_test;

    #[traced_test]
    #[test]
    fn test_file_logging_disabled() {
        let config = FileLoggingFlags {
            logfile_debug_level: Level::INFO,
            logfile_max_size: 0, // Disabled
            logfile_max_number: 100,
            logfile_dir: None,
            logfile_compression: false,
            logfile_color: false,
        };

        let result = init_file_logging(Path::new("."), config);
        assert!(result.is_none());
    }

    #[traced_test]
    #[test]
    fn test_count_layer() {
        // CountLayer will track these events
        tracing::info!("Test info message");
        tracing::warn!("Test warning");
        
        // Metrics would be incremented
        // (actual assertion would require metrics backend)
    }
}
```