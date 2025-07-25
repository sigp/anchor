# Processor Component - Usage Examples

## Basic Setup

### 1. Initialize Processor
```rust
use processor::{Config, spawn};
use task_executor::TaskExecutor;

// Basic configuration with defaults
let config = Config::default();
let executor = TaskExecutor::new();
let senders = spawn(config, executor);
```

### 2. Custom Configuration
```rust
use std::collections::HashMap;
use processor::{Config, QueueKind, spawn};

let mut config = Config {
    max_workers: 8,  // Limit concurrent workers
    queue_size: HashMap::new(),
};

// Override default queue sizes
config.queue_size.insert(QueueKind::UrgentConsensus, 500);
config.queue_size.insert(QueueKind::Permitless, 2000);

let senders = spawn(config, executor);
```

## Submitting Work Items

### 1. Async Tasks
```rust
// Submit an async task
let future = async {
    println!("Processing async work");
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    println!("Async work completed");
};

senders.urgent_consensus
    .send_async(future, "consensus_validation")
    .expect("Queue should not be full");
```

### 2. Blocking Tasks
```rust
// Submit CPU-intensive blocking work
let blocking_work = || {
    // Simulate expensive computation
    std::thread::sleep(std::time::Duration::from_millis(50));
    println!("Heavy computation completed");
};

senders.permitless
    .send_blocking(blocking_work, "crypto_verification")
    .expect("Queue should not be full");
```

### 3. Immediate Tasks
```rust
// Submit immediate task with state access
let immediate_work = |drop_on_finish: processor::work::DropOnFinish| {
    println!("Immediate work executing");
    // Perform quick operations
    // drop_on_finish automatically handles cleanup
};

senders.urgent_consensus
    .send_immediate(immediate_work, "state_update")
    .expect("Queue should not be full");
```

## Advanced Usage

### 1. Work Items with Expiry
```rust
use tokio::time::{Instant, Duration};

// Create work item that expires in 5 seconds
let mut work_item = processor::work::WorkItem::new_async(
    "time_sensitive_task",
    async { println!("Time-sensitive work"); }
);

work_item.set_expiry(Some(Instant::now() + Duration::from_secs(5)));

senders.urgent_consensus
    .send_work_item(work_item)
    .expect("Queue should not be full");
```

### 2. Using Builder Pattern for Expiry
```rust
let work_item = processor::work::WorkItem::new_blocking(
    "batch_processing",
    || println!("Batch work")
).with_expiry(Instant::now() + Duration::from_secs(10));

senders.permitless
    .send_work_item(work_item)
    .expect("Queue should not be full");
```

### 3. Error Handling
```rust
use processor::Error;

match senders.urgent_consensus.send_async(some_future, "critical_task") {
    Ok(()) => println!("Task submitted successfully"),
    Err(Error::Queue(err)) => {
        match err {
            tokio::sync::mpsc::error::TrySendError::Full(_) => {
                println!("Queue is full, task dropped");
            },
            tokio::sync::mpsc::error::TrySendError::Closed(_) => {
                println!("Processor has shut down");
            }
        }
    }
}
```

### 4. Queue Selection
```rust
// Access specific queue
let permitless_sender = senders.get(QueueKind::Permitless);
let urgent_sender = senders.get(QueueKind::UrgentConsensus);

// Submit to appropriate queue based on priority
if is_urgent_consensus_work {
    urgent_sender.send_async(task, "urgent_consensus").ok();
} else {
    permitless_sender.send_async(task, "background_task").ok();
}
```

## Integration Patterns

### 1. Service Integration
```rust
pub struct ConsensusService {
    processor_senders: processor::Senders,
}

impl ConsensusService {
    pub fn process_message(&self, message: ConsensusMessage) {
        let future = async move {
            // Validate and process consensus message
            validate_consensus_message(&message).await;
            apply_consensus_state(&message).await;
        };
        
        self.processor_senders
            .urgent_consensus
            .send_async(future, "consensus_message_processing")
            .unwrap_or_else(|e| {
                tracing::warn!("Failed to submit consensus work: {}", e);
            });
    }
}
```

### 2. Batch Processing
```rust
pub fn process_batch(senders: &processor::Senders, items: Vec<WorkItem>) {
    for (i, item) in items.into_iter().enumerate() {
        let task_name = format!("batch_item_{}", i);
        let future = async move {
            process_single_item(item).await;
        };
        
        senders.permitless
            .send_async(future, Box::leak(task_name.into_boxed_str()))
            .unwrap_or_else(|e| {
                tracing::error!("Failed to submit batch item: {}", e);
            });
    }
}
```

### 3. State Management with Immediate Tasks
```rust
pub fn update_system_state(
    senders: &processor::Senders, 
    state_update: StateUpdate
) {
    let immediate_task = move |drop_on_finish: processor::work::DropOnFinish| {
        // Quick state update that doesn't block
        apply_state_update(state_update);
        
        // If triggering additional work, pass drop_on_finish along
        if needs_follow_up_work() {
            trigger_follow_up_work(drop_on_finish);
        }
        // Otherwise, drop_on_finish is automatically dropped here
    };
    
    senders.urgent_consensus
        .send_immediate(immediate_task, "state_update")
        .expect("Critical state update should not fail");
}
```

## Best Practices

### 1. Task Naming
```rust
// Use descriptive, static names for metrics
senders.permitless.send_async(task, "peer_discovery").ok();
senders.urgent_consensus.send_async(task, "block_validation").ok();
```

### 2. Queue Selection Guidelines
```rust
// Use permitless for:
// - High-frequency, lightweight operations  
// - Tasks that should never be rate-limited
senders.permitless.send_async(log_event(), "event_logging").ok();

// Use urgent_consensus for:
// - Time-sensitive consensus operations
// - Tasks that need coordinated execution limits  
senders.urgent_consensus.send_async(validate_block(), "block_validation").ok();
```

### 3. Resource Management
```rust
// For immediate tasks that trigger more work
let immediate_work = |drop_on_finish: processor::work::DropOnFinish| {
    // Do immediate work
    let result = quick_computation();
    
    // Pass drop_on_finish to async work if needed
    if let Some(follow_up) = result.follow_up_work {
        tokio::spawn(async move {
            follow_up.await;
            drop(drop_on_finish); // Ensure proper cleanup
        });
    }
    // Otherwise drop_on_finish cleans up automatically
};
```