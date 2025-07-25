# Message Receiver Usage Examples

## Basic Setup

### Creating a NetworkMessageReceiver

```rust
use std::sync::Arc;
use tokio::sync::{mpsc, watch};
use message_receiver::{NetworkMessageReceiver, Outcome};
use database::NetworkState;

// Assume we have these components initialized
let processor_senders = get_processor_senders();
let qbft_manager = Arc::new(get_qbft_manager());
let signature_collector = Arc::new(get_signature_collector());
let validator = Arc::new(get_message_validator());

// Create channels for network state and outcomes
let (network_state_tx, network_state_rx) = watch::channel(NetworkState::default());
let (outcome_tx, outcome_rx) = mpsc::channel::<Outcome>(1000);

// Create the message receiver
let message_receiver = NetworkMessageReceiver::new(
    processor_senders,
    qbft_manager,
    signature_collector,
    network_state_rx,
    outcome_tx,
    validator,
);
```

## Integration with Network Layer

### Handling Gossipsub Messages

```rust
use gossipsub::{Message, MessageId};
use libp2p::PeerId;
use message_receiver::MessageReceiver;

async fn handle_gossipsub_message(
    receiver: Arc<NetworkMessageReceiver<impl SlotClock, impl DutiesProvider>>,
    peer_id: PeerId,
    message_id: MessageId,
    message: Message,
) {
    match receiver.receive(peer_id, message_id.clone(), message) {
        Ok(()) => {
            println!("Message {} received successfully", message_id);
        }
        Err(e) => {
            eprintln!("Failed to process message {}: {}", message_id, e);
        }
    }
}
```

### Processing Validation Outcomes

```rust
use tokio::sync::mpsc;
use gossipsub::MessageAcceptance;

async fn process_validation_outcomes(mut outcome_rx: mpsc::Receiver<Outcome>) {
    while let Some(outcome) = outcome_rx.recv().await {
        match outcome.action {
            MessageAcceptance::Accept => {
                println!("Message {} from {:?} accepted", 
                    outcome.message_id, outcome.propagation_source);
                // Forward to gossipsub for propagation
            }
            MessageAcceptance::Reject => {
                println!("Message {} from {:?} rejected", 
                    outcome.message_id, outcome.propagation_source);
                // Tell gossipsub to reject and potentially penalize peer
            }
            MessageAcceptance::Ignore => {
                println!("Message {} from {:?} ignored", 
                    outcome.message_id, outcome.propagation_source);
                // Don't propagate but don't penalize
            }
        }
    }
}
```

## Network State Management

### Updating Validator Shares

```rust
use database::{NetworkState, Share, ValidatorIndex};
use tokio::sync::watch;

async fn update_network_state(
    network_state_tx: watch::Sender<NetworkState>,
    validator_index: ValidatorIndex,
    share: Share,
) {
    let current_state = network_state_tx.borrow().clone();
    let mut new_state = current_state;
    
    // Add or update validator share
    new_state.shares_mut().insert(validator_index, share);
    
    // Send update - this will notify all receivers including message_receiver
    if let Err(_) = network_state_tx.send(new_state) {
        eprintln!("Failed to update network state - no receivers");
    }
}
```

### Committee Membership Updates

```rust
use database::{Cluster, CommitteeId, OperatorId};

async fn update_committee_membership(
    network_state_tx: watch::Sender<NetworkState>,
    committee_id: CommitteeId,
    cluster: Cluster,
) {
    let current_state = network_state_tx.borrow().clone();
    let mut new_state = current_state;
    
    // Update cluster information
    new_state.clusters_mut().insert(committee_id, cluster);
    
    if let Err(_) = network_state_tx.send(new_state) {
        eprintln!("Failed to update committee membership");
    }
}
```

## Message Types and Routing

### QBFT Message Handling

```rust
// The message receiver automatically routes QBFT messages
// Here's what happens internally when a QBFT message arrives:

// 1. Message is validated
// 2. Interest filtering checks if we're a validator/committee member
// 3. If interested, message is routed to qbft_manager:

async fn example_qbft_flow() {
    // This is handled automatically by NetworkMessageReceiver
    // When it receives a ValidatedSSVMessage::QbftMessage:
    
    // qbft_manager.receive_data(signed_ssv_message, qbft_message)?;
    
    // Your qbft_manager should implement proper handling:
    println!("QBFT message received and processed by manager");
}
```

### Partial Signature Message Handling

```rust
// Similarly, partial signature messages are automatically routed
// to the signature collector:

async fn example_signature_flow() {
    // This is handled automatically by NetworkMessageReceiver
    // When it receives ValidatedSSVMessage::PartialSignatureMessages:
    
    // signature_collector.receive_partial_signatures(messages)?;
    
    // Your signature collector should aggregate the signatures:
    println!("Partial signature received and processed by collector");
}
```

## Error Handling Patterns

### Robust Message Processing

```rust
use tracing::{error, debug};

async fn robust_message_handling(
    receiver: Arc<NetworkMessageReceiver<impl SlotClock, impl DutiesProvider>>,
    peer_id: PeerId,
    message_id: MessageId,
    message: Message,
) {
    match receiver.receive(peer_id, message_id.clone(), message) {
        Ok(()) => {
            debug!("Successfully processed message {}", message_id);
        }
        Err(message_receiver::Error::Processor(processor_err)) => {
            error!("Processor error for message {}: {}", message_id, processor_err);
            // The processor might be overloaded, consider backpressure
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }
}
```

### Channel Monitoring

```rust
use tokio::sync::mpsc::error::TryRecvError;

async fn monitor_outcome_channel(mut outcome_rx: mpsc::Receiver<Outcome>) {
    loop {
        match outcome_rx.try_recv() {
            Ok(outcome) => {
                // Process outcome
                handle_outcome(outcome).await;
            }
            Err(TryRecvError::Empty) => {
                // No messages available, this is normal
                tokio::time::sleep(std::time::Duration::from_millis(1)).await;
            }
            Err(TryRecvError::Disconnected) => {
                error!("Outcome channel disconnected - message receiver may have stopped");
                break;
            }
        }
    }
}

async fn handle_outcome(outcome: Outcome) {
    // Implement your outcome handling logic
    println!("Handling outcome for message {}", outcome.message_id);
}
```

## Testing Patterns

### Mock Setup for Testing

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::sync::{mpsc, watch};
    
    fn create_test_receiver() -> (
        Arc<NetworkMessageReceiver<TestSlotClock, TestDutiesProvider>>,
        mpsc::Receiver<Outcome>,
        watch::Sender<NetworkState>,
    ) {
        let processor = create_mock_processor();
        let qbft_manager = Arc::new(create_mock_qbft_manager());
        let signature_collector = Arc::new(create_mock_signature_collector());
        let validator = Arc::new(create_mock_validator());
        
        let (network_state_tx, network_state_rx) = watch::channel(NetworkState::default());
        let (outcome_tx, outcome_rx) = mpsc::channel(100);
        
        let receiver = NetworkMessageReceiver::new(
            processor,
            qbft_manager,
            signature_collector,
            network_state_rx,
            outcome_tx,
            validator,
        );
        
        (receiver, outcome_rx, network_state_tx)
    }
    
    #[tokio::test]
    async fn test_message_reception() {
        let (receiver, mut outcome_rx, _state_tx) = create_test_receiver();
        
        let test_message = create_test_gossipsub_message();
        let peer_id = PeerId::random();
        let message_id = MessageId::from(b"test_message".to_vec());
        
        // Send message to receiver
        let result = receiver.receive(peer_id, message_id.clone(), test_message);
        assert!(result.is_ok());
        
        // Check that we got an outcome
        let outcome = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            outcome_rx.recv()
        ).await.expect("Timeout waiting for outcome")
             .expect("No outcome received");
        
        assert_eq!(outcome.message_id, message_id);
        assert_eq!(outcome.propagation_source, peer_id);
    }
}
```

## Performance Optimization

### Batch Processing Outcomes

```rust
use std::collections::VecDeque;
use tokio::time::{interval, Duration};

async fn batch_process_outcomes(mut outcome_rx: mpsc::Receiver<Outcome>) {
    let mut batch = VecDeque::new();
    let mut ticker = interval(Duration::from_millis(10));
    
    loop {
        tokio::select! {
            // Collect outcomes as they arrive
            outcome = outcome_rx.recv() => {
                match outcome {
                    Some(outcome) => batch.push_back(outcome),
                    None => break, // Channel closed
                }
            }
            
            // Process batch periodically
            _ = ticker.tick() => {
                if !batch.is_empty() {
                    process_outcome_batch(&mut batch).await;
                }
            }
        }
    }
}

async fn process_outcome_batch(batch: &mut VecDeque<Outcome>) {
    while let Some(outcome) = batch.pop_front() {
        // Process individual outcome
        handle_outcome(outcome).await;
    }
}
```

This completes the usage examples showing how to integrate and use the message receiver component effectively in various scenarios.