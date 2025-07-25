# Eth Component - Usage Examples

## Basic Setup and Initialization

### Creating an SsvEventSyncer for Node Operation

```rust
use std::sync::Arc;
use eth::{Config, SsvEventSyncer};
use database::NetworkDatabase;
use ssv_network_config::SsvNetworkConfig;
use sensitive_url::SensitiveUrl;

async fn setup_event_syncer() -> Result<SsvEventSyncer, ExecutionError> {
    // Database connection
    let db = Arc::new(NetworkDatabase::new("path/to/db").await?);
    
    // Communication channels
    let (index_sync_tx, _) = tokio::sync::mpsc::channel(100);
    let (exit_tx, _) = tokio::sync::mpsc::channel(100);
    
    // Configuration
    let config = Config {
        http_urls: vec![
            "https://mainnet.infura.io/v3/YOUR_KEY".parse()?,
            "https://eth-mainnet.alchemyapi.io/v2/YOUR_KEY".parse()?,
        ],
        ws_url: "wss://mainnet.infura.io/ws/v3/YOUR_KEY".parse()?,
        network: SsvNetworkConfig::mainnet(),
    };
    
    // Create syncer
    let syncer = SsvEventSyncer::new(db, index_sync_tx, exit_tx, config).await?;
    Ok(syncer)
}
```

### Creating a KeySplit Syncer

```rust
use eth::SsvEventSyncer;

fn setup_keysplit_syncer() -> SsvEventSyncer {
    let db = Arc::new(database);
    let rpc_endpoint = "https://mainnet.infura.io/v3/YOUR_KEY".to_string();
    let network = SsvNetworkConfig::mainnet();
    
    SsvEventSyncer::new_keysplit(db, rpc_endpoint, network)
}
```

## Event Processing Examples

### Processing SSV Contract Events

```rust
use eth::event_processor::{EventProcessor, Mode};
use alloy::rpc::types::Log;

async fn process_events_example() {
    let db = Arc::new(database);
    let (index_sync_tx, mut index_sync_rx) = tokio::sync::mpsc::channel(100);
    let (exit_tx, mut exit_rx) = tokio::sync::mpsc::channel(100);
    
    // Create processor for node operation
    let processor = EventProcessor::new(
        db.clone(),
        Mode::Node { index_sync_tx, exit_tx }
    );
    
    // Sample logs from Ethereum
    let logs: Vec<Log> = fetch_contract_logs().await;
    
    // Process the logs
    match processor.process_logs(logs, true, current_block) {
        Ok(_) => println!("Events processed successfully"),
        Err(e) => eprintln!("Error processing events: {}", e),
    }
    
    // Handle side effects
    tokio::spawn(async move {
        while let Some(validator_request) = index_sync_rx.recv().await {
            // Process validator index sync request
            handle_index_sync(validator_request).await;
        }
    });
    
    tokio::spawn(async move {
        while let Some(exit_request) = exit_rx.recv().await {
            // Process validator exit request
            handle_exit_request(exit_request).await;
        }
    });
}
```

### Handling Specific Event Types

```rust
use eth::generated::SSVContract;
use alloy::sol_types::SolEvent;

// Processing OperatorAdded events
fn handle_operator_added(log: &Log) -> Result<(), ExecutionError> {
    if log.topic0() == Some(&SSVContract::OperatorAdded::SIGNATURE_HASH) {
        let event = SSVContract::OperatorAdded::decode_log_object(log)?;
        
        println!("New operator added:");
        println!("  ID: {}", event.operatorId);
        println!("  Owner: {:?}", event.owner);
        println!("  Public Key: {:?}", event.publicKey);
        println!("  Fee: {}", event.fee);
        
        // Store in database
        store_operator(&event)?;
    }
    Ok(())
}

// Processing ValidatorAdded events
fn handle_validator_added(log: &Log) -> Result<(), ExecutionError> {
    if log.topic0() == Some(&SSVContract::ValidatorAdded::SIGNATURE_HASH) {
        let event = SSVContract::ValidatorAdded::decode_log_object(log)?;
        
        println!("New validator added:");
        println!("  Owner: {:?}", event.owner);
        println!("  Operator IDs: {:?}", event.operatorIds);
        println!("  Public Key: {:?}", event.publicKey);
        
        // Trigger index sync for new validator
        if let Mode::Node { index_sync_tx, .. } = &processor.mode {
            let request = IndexSyncRequest {
                pubkey: event.publicKey,
                operator_ids: event.operatorIds,
            };
            index_sync_tx.send(request).await?;
        }
    }
    Ok(())
}
```

## Synchronization Patterns

### Historical Sync with Error Handling

```rust
async fn perform_historical_sync(syncer: &mut SsvEventSyncer) {
    let contract_address = syncer.network.ssv_contract;
    let deployment_block = syncer.network.ssv_contract_block;
    
    loop {
        match syncer.historical_sync(contract_address, deployment_block, SSV_EVENTS).await {
            Ok(_) => {
                println!("Historical sync completed successfully");
                break;
            }
            Err(ExecutionError::RpcError(e)) => {
                eprintln!("RPC error during sync: {}", e);
                // Exponential backoff
                tokio::time::sleep(Duration::from_millis(1000)).await;
                continue;
            }
            Err(e) => {
                eprintln!("Fatal sync error: {}", e);
                break;
            }
        }
    }
}
```

### Live Event Monitoring

```rust
use tokio::select;

async fn monitor_live_events(syncer: &mut SsvEventSyncer) {
    let mut shutdown_rx = setup_shutdown_signal();
    
    loop {
        select! {
            result = syncer.start_live_sync() => {
                match result {
                    Ok(_) => println!("Live sync completed"),
                    Err(e) => {
                        eprintln!("Live sync error: {}", e);
                        // Reconnect after delay
                        tokio::time::sleep(Duration::from_secs(5)).await;
                    }
                }
            }
            _ = shutdown_rx.recv() => {
                println!("Shutting down event monitoring");
                break;
            }
        }
    }
}
```

## Error Handling Patterns

### Comprehensive Error Handling

```rust
use eth::error::ExecutionError;

async fn robust_event_processing(processor: &EventProcessor, logs: Vec<Log>) {
    match processor.process_logs(logs, true, current_block) {
        Ok(_) => {
            metrics::increment_counter(&metrics::EXECUTION_EVENTS_PROCESSED, logs.len());
        }
        Err(ExecutionError::Database(e)) => {
            eprintln!("Database error: {}", e);
            // Possibly retry or queue for later
            schedule_retry(logs).await;
        }
        Err(ExecutionError::InvalidEvent(e)) => {
            eprintln!("Invalid event format: {}", e);
            // Log and skip invalid events
            metrics::increment_counter("invalid_events", 1);
        }
        Err(ExecutionError::RpcError(e)) => {
            eprintln!("RPC communication error: {}", e);
            // Switch to fallback endpoint
            switch_to_fallback_rpc().await;
        }
        Err(e) => {
            eprintln!("Unexpected error: {}", e);
            // General error handling
        }
    }
}
```

## Configuration Examples

### Production Configuration

```rust
use sensitive_url::SensitiveUrl;

fn production_config() -> Config {
    Config {
        http_urls: vec![
            "https://mainnet.infura.io/v3/PROJECT_ID".parse().unwrap(),
            "https://eth-mainnet.alchemyapi.io/v2/API_KEY".parse().unwrap(),
            "https://rpc.ankr.com/eth".parse().unwrap(),
        ],
        ws_url: "wss://mainnet.infura.io/ws/v3/PROJECT_ID".parse().unwrap(),
        network: SsvNetworkConfig::mainnet(),
    }
}
```

### Testnet Configuration

```rust
fn testnet_config() -> Config {
    Config {
        http_urls: vec![
            "https://goerli.infura.io/v3/PROJECT_ID".parse().unwrap(),
        ],
        ws_url: "wss://goerli.infura.io/ws/v3/PROJECT_ID".parse().unwrap(),
        network: SsvNetworkConfig::goerli(),
    }
}
```

## Integration with Other Components

### Database Integration

```rust
use database::{NetworkDatabase, UniqueIndex};
use rusqlite::Transaction;

fn store_operator_with_transaction(
    event: &SSVContract::OperatorAdded,
    tx: &Transaction,
) -> Result<(), ExecutionError> {
    let operator = Operator {
        id: event.operatorId.into(),
        owner: event.owner,
        public_key: event.publicKey.clone(),
        fee: event.fee,
        active: true,
    };
    
    // Store in database within transaction
    match tx.insert_operator(&operator) {
        Ok(_) => Ok(()),
        Err(database::Error::Conflict(_)) => {
            // Handle duplicate operator
            Err(ExecutionError::Duplicate("Operator already exists".into()))
        }
        Err(e) => Err(ExecutionError::Database(e.to_string())),
    }
}
```

### Metrics Collection

```rust
use eth::metrics;

fn collect_processing_metrics(event_count: usize, processing_time: Duration) {
    // Update event processing metrics
    metrics::increment_counter(&metrics::EXECUTION_EVENTS_PROCESSED, event_count);
    metrics::observe_histogram(&metrics::EXECUTION_LOG_PROCESSING_TIME, processing_time);
    
    // Update sync status
    metrics::set_gauge(&metrics::EXECUTION_SYNC_STATUS, 1.0);
    
    // Custom metrics
    metrics::increment_counter("eth_events_by_type", 1, &[("type", "operator_added")]);
}
```

## Testing Patterns

### Mock Event Processing

```rust
#[cfg(test)]
mod tests {
    use super::*;
    
    fn create_mock_operator_added_log() -> Log {
        // Create mock log for testing
        Log {
            address: SSV_CONTRACT_ADDRESS,
            topics: vec![SSVContract::OperatorAdded::SIGNATURE_HASH.into()],
            data: encode_operator_added_data(1, owner_address, public_key, fee),
            block_number: Some(12345),
            transaction_hash: Some(tx_hash),
            // ... other fields
        }
    }
    
    #[tokio::test]
    async fn test_operator_added_processing() {
        let db = Arc::new(test_database().await);
        let processor = EventProcessor::new(db.clone(), Mode::KeySplit);
        
        let log = create_mock_operator_added_log();
        let result = processor.process_logs(vec![log], false, 12345);
        
        assert!(result.is_ok());
        
        // Verify operator was stored
        let operators = db.get_operators().await.unwrap();
        assert_eq!(operators.len(), 1);
        assert_eq!(operators[0].id, OperatorId::from(1));
    }
}
```