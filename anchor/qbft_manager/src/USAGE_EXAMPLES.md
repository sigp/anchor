# QBFT Manager Usage Examples

## Basic Setup

### Creating a QbftManager

```rust
use std::sync::Arc;
use qbft_manager::{QbftManager, QbftError};
use processor::Senders;
use database::OwnOperatorId;
use slot_clock::SystemTimeSlotClock;
use message_sender::NetworkMessageSender;
use ssv_types::domain_type::DomainType;

async fn setup_qbft_manager() -> Result<Arc<QbftManager>, QbftError> {
    // Setup dependencies
    let processor = Senders::new(/* processor config */);
    let operator_id = OwnOperatorId::new(42);
    let slot_clock = SystemTimeSlotClock::new(/* genesis time, slot duration */);
    let message_sender = Arc::new(NetworkMessageSender::new(/* network config */));
    let domain = DomainType::BeaconProposer;

    // Create the manager
    QbftManager::new(
        processor,
        operator_id,
        slot_clock,
        message_sender,
        domain,
    )
}
```

## Validator Consensus Examples

### Starting a Proposal Consensus

```rust
use qbft_manager::{QbftManager, ValidatorInstanceId, ValidatorDutyKind};
use ssv_types::{
    Cluster, OperatorId,
    consensus::ValidatorConsensusData,
};
use qbft::InstanceHeight;
use tokio::time::Instant;
use types::PublicKeyBytes;

async fn start_proposal_consensus(
    manager: &QbftManager,
    validator_pubkey: PublicKeyBytes,
    proposal_data: ValidatorConsensusData,
    committee: &Cluster,
) -> Result<(), Box<dyn std::error::Error>> {
    let instance_id = ValidatorInstanceId {
        validator: validator_pubkey,
        duty: ValidatorDutyKind::Proposal,
        instance_height: InstanceHeight::from(12345),
    };

    let start_time = Instant::now();
    
    let result = manager.decide_instance(
        instance_id,
        proposal_data,
        start_time,
        committee,
    ).await?;

    match result {
        qbft::Completed::Success(data) => {
            println!("Proposal consensus succeeded: {:?}", data);
        }
        qbft::Completed::TimedOut => {
            println!("Proposal consensus timed out");
        }
    }

    Ok(())
}
```

### Handling Aggregation Duties

```rust
async fn start_aggregation_consensus(
    manager: &QbftManager,
    validator_pubkey: PublicKeyBytes,
    aggregation_data: ValidatorConsensusData,
    committee: &Cluster,
) -> Result<(), Box<dyn std::error::Error>> {
    let instance_id = ValidatorInstanceId {
        validator: validator_pubkey,
        duty: ValidatorDutyKind::Aggregator,
        instance_height: InstanceHeight::from(12346),
    };

    let start_time = Instant::now() + Duration::from_secs(5); // Delayed start
    
    let result = manager.decide_instance(
        instance_id,
        aggregation_data,
        start_time,
        committee,
    ).await?;

    println!("Aggregation result: {:?}", result);
    Ok(())
}
```

## Committee Consensus Examples

### Beacon Vote Consensus

```rust
use ssv_types::{
    CommitteeId,
    consensus::BeaconVote,
};
use qbft_manager::CommitteeInstanceId;

async fn start_beacon_vote_consensus(
    manager: &QbftManager,
    committee_id: CommitteeId,
    beacon_vote: BeaconVote,
    committee: &Cluster,
) -> Result<(), Box<dyn std::error::Error>> {
    let instance_id = CommitteeInstanceId {
        committee: committee_id,
        instance_height: InstanceHeight::from(54321),
    };

    let start_time = Instant::now();
    
    let result = manager.decide_instance(
        instance_id,
        beacon_vote,
        start_time,
        committee,
    ).await?;

    match result {
        qbft::Completed::Success(vote) => {
            println!("Beacon vote consensus completed: {:?}", vote);
        }
        qbft::Completed::TimedOut => {
            println!("Beacon vote consensus timed out");
        }
    }

    Ok(())
}
```

## Network Message Handling

### Processing Incoming QBFT Messages

```rust
use ssv_types::{
    message::SignedSSVMessage,
    consensus::QbftMessage,
};

async fn handle_network_message(
    manager: &QbftManager,
    signed_message: SignedSSVMessage,
    qbft_message: QbftMessage,
) -> Result<(), Box<dyn std::error::Error>> {
    match manager.receive_data(signed_message, qbft_message) {
        Ok(()) => {
            println!("Message successfully routed to QBFT instance");
        }
        Err(qbft_manager::QbftError::InconsistentMessageId) => {
            println!("Message had invalid or inconsistent message ID");
        }
        Err(e) => {
            println!("Error processing message: {:?}", e);
        }
    }
    
    Ok(())
}
```

## Error Handling Patterns

### Robust Consensus Execution

```rust
use qbft_manager::QbftError;

async fn robust_consensus_example(
    manager: &QbftManager,
    instance_id: ValidatorInstanceId,
    initial_data: ValidatorConsensusData,
    committee: &Cluster,
) -> Result<(), Box<dyn std::error::Error>> {
    let start_time = Instant::now();
    
    match manager.decide_instance(instance_id, initial_data, start_time, committee).await {
        Ok(qbft::Completed::Success(data)) => {
            println!("Consensus successful: {:?}", data);
        }
        Ok(qbft::Completed::TimedOut) => {
            println!("Consensus timed out - network may be partitioned");
            // Implement retry logic or fallback behavior
        }
        Err(QbftError::OwnOperatorIdUnknown) => {
            println!("Node operator ID not configured");
            // Handle configuration error
        }
        Err(QbftError::QueueFullError) => {
            println!("Processor queue full - system overloaded");
            // Implement backpressure handling
        }
        Err(QbftError::QueueClosedError) => {
            println!("Processor shut down");
            // Handle graceful shutdown
        }
        Err(e) => {
            println!("Other error: {:?}", e);
        }
    }
    
    Ok(())
}
```

## Testing Examples

### Mock Setup for Tests

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use qbft_manager::tests::TestContext;
    use message_sender::testing::MockMessageSender;
    use slot_clock::ManualSlotClock;
    
    #[tokio::test]
    async fn test_consensus_flow() {
        let mut test_context = TestContext::new().await;
        
        // Setup test committee
        let committee = Cluster {
            cluster_members: vec![1, 2, 3, 4].into_iter().collect(),
            // ... other fields
        };
        
        // Create test data
        let validator_pubkey = PublicKeyBytes::from([1; 48]);
        let consensus_data = ValidatorConsensusData {
            // ... test data
        };
        
        let instance_id = ValidatorInstanceId {
            validator: validator_pubkey,
            duty: ValidatorDutyKind::Proposal,
            instance_height: InstanceHeight::from(1),
        };
        
        // Execute consensus
        let result = test_context.manager.decide_instance(
            instance_id,
            consensus_data,
            Instant::now(),
            &committee,
        ).await;
        
        assert!(result.is_ok());
    }
}
```

### Concurrent Consensus Testing

```rust
#[tokio::test]
async fn test_concurrent_consensus() {
    let test_context = TestContext::new().await;
    let manager = &test_context.manager;
    
    // Start multiple concurrent consensus instances
    let mut handles = vec![];
    
    for i in 0..5 {
        let manager = manager.clone();
        let committee = test_committee();
        
        let handle = tokio::spawn(async move {
            let instance_id = ValidatorInstanceId {
                validator: PublicKeyBytes::from([i; 48]),
                duty: ValidatorDutyKind::Proposal,
                instance_height: InstanceHeight::from(i as usize),
            };
            
            let data = create_test_consensus_data(i);
            manager.decide_instance(instance_id, data, Instant::now(), &committee).await
        });
        
        handles.push(handle);
    }
    
    // Wait for all consensus instances to complete
    for handle in handles {
        let result = handle.await.expect("Task failed");
        assert!(result.is_ok());
    }
}
```

## Best Practices

### Resource Management

```rust
// Always use Arc for shared access
let manager = Arc::new(QbftManager::new(/* ... */)?);

// Clone Arc for concurrent access
let manager_clone = Arc::clone(&manager);
tokio::spawn(async move {
    // Use manager_clone in async task
});
```

### Committee Configuration

```rust
use ssv_types::{Cluster, IndexSet};

fn create_committee(operator_ids: Vec<u64>) -> Cluster {
    Cluster {
        cluster_members: operator_ids.into_iter().collect::<IndexSet<_>>(),
        // Configure for 3f+1 Byzantine tolerance
        // For 4 operators: f=1, so we need 3 signatures for quorum
    }
}
```

### Message ID Construction

```rust
use ssv_types::{
    msgid::{MessageId, Role, DutyExecutor},
    domain_type::DomainType,
};

fn create_message_id(
    domain: &DomainType,
    validator: PublicKeyBytes,
    role: Role,
) -> MessageId {
    MessageId::new(
        domain,
        role,
        &DutyExecutor::Validator(validator),
    )
}
```

### Timeout Handling

```rust
use tokio::time::{timeout, Duration};

async fn consensus_with_timeout(
    manager: &QbftManager,
    instance_id: ValidatorInstanceId,
    data: ValidatorConsensusData,
    committee: &Cluster,
) -> Result<qbft::Completed<ValidatorConsensusData>, Box<dyn std::error::Error>> {
    let consensus_future = manager.decide_instance(
        instance_id,
        data,
        Instant::now(),
        committee,
    );
    
    // Add application-level timeout
    match timeout(Duration::from_secs(300), consensus_future).await {
        Ok(result) => Ok(result?),
        Err(_) => {
            println!("Application timeout exceeded");
            Err("Consensus timeout".into())
        }
    }
}
```