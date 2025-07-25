# Message Sender Usage Examples

## Basic Setup and Initialization

### Creating a NetworkMessageSender

```rust
use std::sync::Arc;
use openssl::rsa::Rsa;
use tokio::sync::{mpsc, watch};
use database::OwnOperatorId;
use message_sender::NetworkMessageSender;
use subnet_service::SubnetId;

async fn setup_network_sender() -> Result<Arc<NetworkMessageSender<MySlotClock, MyDutiesProvider>>, String> {
    // Setup processor channels
    let processor = processor::Senders::new();
    
    // Create network transmission channel
    let (network_tx, network_rx) = mpsc::channel::<(SubnetId, Vec<u8>)>(1000);
    
    // Generate RSA key pair
    let rsa = Rsa::generate(2048).unwrap();
    
    // Setup operator ID
    let operator_id = OwnOperatorId::new();
    operator_id.set(42); // Set your operator ID
    
    // Create sync status watcher
    let (sync_tx, sync_rx) = watch::channel(true);
    
    // Setup optional validator
    let validator = Some(Arc::new(create_validator()));
    
    NetworkMessageSender::new(
        processor,
        network_tx,
        rsa,
        operator_id,
        validator,
        128, // subnet_count
        sync_rx,
    )
}
```

### Creating Testing Implementations

```rust
use message_sender::{ImpostorMessageSender, MockMessageSender};
use tokio::sync::mpsc;

// For integration testing - logs but doesn't send
fn create_impostor_sender() -> ImpostorMessageSender {
    let (network_tx, _network_rx) = mpsc::channel(100);
    ImpostorMessageSender::new(network_tx, 128)
}

// For unit testing - captures messages
fn create_mock_sender() -> (MockMessageSender, mpsc::UnboundedReceiver<SignedSSVMessage>) {
    let (message_tx, message_rx) = mpsc::unbounded_channel();
    let sender = MockMessageSender::new(message_tx, 42); // operator_id = 42
    (sender, message_rx)
}
```

## Message Sending Operations

### Signing and Sending Unsigned Messages

```rust
use ssv_types::{
    CommitteeId, 
    consensus::UnsignedSSVMessage,
    message::SSVMessage,
};

async fn send_consensus_message(
    sender: &Arc<NetworkMessageSender<SlotClock, DutiesProvider>>
) -> Result<(), message_sender::Error> {
    // Create unsigned SSV message
    let ssv_message = SSVMessage::consensus(/* consensus data */);
    let unsigned_message = UnsignedSSVMessage {
        ssv_message,
        full_data: Some(vec![/* additional data */]),
    };
    
    let committee_id = CommitteeId::new(1, 2, 3, 4); // Example committee
    
    // Optional callback to handle the signed message
    let callback = Some(Box::new(|signed_msg: &SignedSSVMessage| {
        println!("Message signed and ready: {:?}", signed_msg);
    }) as Box<dyn FnOnce(&SignedSSVMessage) + Send>);
    
    // Sign and send the message
    sender.sign_and_send(unsigned_message, committee_id, callback)
}
```

### Sending Pre-signed Messages

```rust
async fn relay_signed_message(
    sender: &Arc<NetworkMessageSender<SlotClock, DutiesProvider>>,
    signed_message: SignedSSVMessage,
) -> Result<(), message_sender::Error> {
    let committee_id = CommitteeId::new(1, 2, 3, 4);
    
    // Send already signed message
    sender.send(signed_message, committee_id)
}
```

## Error Handling Patterns

### Comprehensive Error Handling

```rust
use message_sender::Error;

async fn robust_message_sending(
    sender: &Arc<NetworkMessageSender<SlotClock, DutiesProvider>>,
    message: UnsignedSSVMessage,
    committee_id: CommitteeId,
) {
    match sender.sign_and_send(message, committee_id, None) {
        Ok(_) => {
            println!("Message sent successfully");
        },
        Err(Error::NetworkQueueClosed) => {
            eprintln!("Network is shutting down, cannot send message");
            // Handle graceful shutdown
        },
        Err(Error::OwnOperatorIdUnknown) => {
            eprintln!("Operator ID not configured, cannot sign message");
            // Wait for operator registration or reconfigure
        },
        Err(Error::NotSynced) => {
            println!("Node not synced, waiting to send message");
            // Retry after sync completion
        },
        Err(Error::Processor(proc_err)) => {
            eprintln!("Processor error: {:?}", proc_err);
            // Handle processor-specific errors
        },
    }
}
```

### Retry Logic with Backoff

```rust
use tokio::time::{sleep, Duration};

async fn send_with_retry(
    sender: &Arc<NetworkMessageSender<SlotClock, DutiesProvider>>,
    message: UnsignedSSVMessage,
    committee_id: CommitteeId,
    max_retries: u32,
) -> Result<(), Error> {
    let mut attempts = 0;
    
    loop {
        match sender.sign_and_send(message.clone(), committee_id, None) {
            Ok(_) => return Ok(()),
            Err(Error::NotSynced) if attempts < max_retries => {
                attempts += 1;
                let delay = Duration::from_millis(100 * 2_u64.pow(attempts));
                sleep(delay).await;
                continue;
            },
            Err(err) => return Err(err),
        }
    }
}
```

## Testing Patterns

### Unit Testing with MockMessageSender

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tokio_test;
    
    #[tokio::test]
    async fn test_message_capture() {
        let (mock_sender, mut message_rx) = create_mock_sender();
        let committee_id = CommitteeId::new(1, 2, 3, 4);
        
        // Create test message
        let unsigned_msg = create_test_message();
        
        // Send message
        mock_sender.sign_and_send(unsigned_msg, committee_id, None).unwrap();
        
        // Verify message was captured
        let captured_message = message_rx.recv().await.unwrap();
        assert_eq!(captured_message.operators(), &[42]);
        assert_eq!(captured_message.signatures().len(), 1);
    }
    
    #[tokio::test]
    async fn test_callback_execution() {
        let (mock_sender, _) = create_mock_sender();
        let committee_id = CommitteeId::new(1, 2, 3, 4);
        
        let callback_executed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let callback_flag = callback_executed.clone();
        
        let callback = Some(Box::new(move |_: &SignedSSVMessage| {
            callback_flag.store(true, std::sync::atomic::Ordering::Relaxed);
        }) as Box<dyn FnOnce(&SignedSSVMessage) + Send>);
        
        mock_sender.sign_and_send(create_test_message(), committee_id, callback).unwrap();
        
        assert!(callback_executed.load(std::sync::atomic::Ordering::Relaxed));
    }
}
```

### Integration Testing with ImpostorMessageSender

```rust
#[cfg(test)]
mod integration_tests {
    use super::*;
    
    #[tokio::test]
    async fn test_message_routing() {
        let impostor = create_impostor_sender();
        let committee_id = CommitteeId::new(1, 2, 3, 4);
        
        // This will log debug messages showing subnet calculation
        let result = impostor.sign_and_send(create_test_message(), committee_id, None);
        
        // ImpostorMessageSender always succeeds
        assert!(result.is_ok());
    }
}
```

## Advanced Usage Patterns

### Message Batching

```rust
async fn batch_send_messages(
    sender: &Arc<NetworkMessageSender<SlotClock, DutiesProvider>>,
    messages: Vec<(UnsignedSSVMessage, CommitteeId)>,
) -> Vec<Result<(), Error>> {
    let mut results = Vec::new();
    
    for (message, committee_id) in messages {
        let result = sender.sign_and_send(message, committee_id, None);
        results.push(result);
        
        // Small delay to prevent overwhelming the processor
        sleep(Duration::from_millis(10)).await;
    }
    
    results
}
```

### Custom Message Callbacks

```rust
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

// Message counter for monitoring
static MESSAGE_COUNTER: AtomicU64 = AtomicU64::new(0);

async fn send_with_monitoring(
    sender: &Arc<NetworkMessageSender<SlotClock, DutiesProvider>>,
    message: UnsignedSSVMessage,
    committee_id: CommitteeId,
) -> Result<(), Error> {
    let callback = Some(Box::new(|signed_msg: &SignedSSVMessage| {
        let count = MESSAGE_COUNTER.fetch_add(1, Ordering::Relaxed);
        println!("Sent message #{}: {} bytes", count + 1, signed_msg.as_ssz_bytes().len());
    }) as Box<dyn FnOnce(&SignedSSVMessage) + Send>);
    
    sender.sign_and_send(message, committee_id, callback)
}
```

### Multi-Committee Broadcasting

```rust
async fn broadcast_to_committees(
    sender: &Arc<NetworkMessageSender<SlotClock, DutiesProvider>>,
    message: UnsignedSSVMessage,
    committees: Vec<CommitteeId>,
) -> Result<(), Error> {
    let message = Arc::new(message);
    let mut handles = Vec::new();
    
    for committee_id in committees {
        let sender = sender.clone();
        let message = message.clone();
        
        let handle = tokio::spawn(async move {
            sender.sign_and_send((*message).clone(), committee_id, None)
        });
        
        handles.push(handle);
    }
    
    // Wait for all sends to complete
    for handle in handles {
        handle.await.map_err(|_| Error::NetworkQueueClosed)??;
    }
    
    Ok(())
}
```

## Configuration Examples

### Production Configuration

```rust
// In your main application setup
async fn setup_production_message_sender() -> Arc<NetworkMessageSender<ProductionSlotClock, ProductionDutiesProvider>> {
    let config = load_config();
    
    let processor = processor::Senders::new();
    let (network_tx, network_rx) = mpsc::channel(config.network_queue_size);
    
    // Load operator's private key
    let private_key_pem = std::fs::read(&config.private_key_path).unwrap();
    let rsa = Rsa::private_key_from_pem(&private_key_pem).unwrap();
    
    let operator_id = OwnOperatorId::new();
    operator_id.set(config.operator_id);
    
    let validator = Some(Arc::new(Validator::new(
        duties_provider,
        slot_clock,
        config.validation_settings,
    )));
    
    let (sync_tx, sync_rx) = watch::channel(false);
    
    // Start sync status updater
    tokio::spawn(update_sync_status(sync_tx));
    
    NetworkMessageSender::new(
        processor,
        network_tx,
        rsa,
        operator_id,
        validator,
        config.subnet_count,
        sync_rx,
    ).unwrap()
}
```