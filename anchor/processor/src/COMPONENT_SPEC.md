# Processor Component - Technical Specification

## Component Interface

### Primary Entry Point
```rust
pub fn spawn(config: Config, executor: TaskExecutor) -> Senders
```

### Configuration
```rust
pub struct Config {
    pub max_workers: usize,           // Default: num_cpus::get()
    pub queue_size: HashMap<QueueKind, usize>,  // Custom queue sizes
}
```

## Data Types

### Work Item Types
```rust
pub struct WorkItem {
    func: WorkKind,
    expiry: Option<Instant>,
    name: &'static str,
}

pub enum WorkKind {
    Async(Pin<Box<dyn Future<Output = ()> + Send>>),
    Blocking(Box<dyn FnOnce() + Send>),
    Immediate(Box<dyn FnOnce(DropOnFinish) + Send>),
}
```

### Queue Types
```rust
pub enum QueueKind {
    Permitless,        // Default size: 1000
    UrgentConsensus,   // Default size: 1000
}
```

### Senders Interface
```rust
pub struct Senders {
    pub permitless: Sender,
    pub urgent_consensus: Sender,
}

impl Sender {
    pub fn send_async<F>(&self, future: F, name: &'static str) -> Result<(), Error>
    pub fn send_blocking<F>(&self, func: F, name: &'static str) -> Result<(), Error>  
    pub fn send_immediate<F>(&self, func: F, name: &'static str) -> Result<(), Error>
    pub fn send_work_item(&self, item: WorkItem) -> Result<(), TrySendError<WorkItem>>
    pub fn is_closed(&self) -> bool
}
```

## Processing Behavior

### Queue Priority Order
1. **Permitless Queue**: Always checked first, bypasses permit system
2. **UrgentConsensus Queue**: Requires permit, high priority for consensus operations

### Permit System
- **Max Workers**: Configurable limit on concurrent permit-holding tasks
- **Permitless Tasks**: Unlimited, don't count toward worker limit
- **Permit Acquisition**: Blocks until permit available for non-permitless queues

### Work Item Lifecycle
1. **Submission**: Non-blocking `try_send` operation
2. **Queue Storage**: FIFO within each queue type
3. **Expiry Check**: Items past expiry time are dropped
4. **Execution**: Dispatched based on WorkKind
5. **Cleanup**: Permits returned, metrics updated via `DropOnFinish`

## Error Handling

### Error Types
```rust
pub enum Error {
    Queue(TrySendError<WorkItem>),
}
```

### Queue Full Behavior
- Returns `TrySendError::Full` 
- Logs warning with task name and queue
- Updates error metrics counter

### Queue Closed Behavior  
- Returns `TrySendError::Closed`
- Logs error message
- Processor loop exits when all queues closed

## Metrics Specification

### Counters
- `anchor_processor_work_events_submitted_count{type, queue}`
- `anchor_processor_work_events_started_count{type}`
- `anchor_processor_work_events_expired_count{type}`
- `anchor_processor_send_error_per_work_type{type, queue}`

### Gauges
- `anchor_processor_workers_active_total`
- `anchor_processor_permit_workers_active_total`
- `anchor_processor_work_event_queue_length{type, queue}`

### Histograms
- `anchor_processor_worker_time{type}` - Task execution duration
- `anchor_processor_event_handling_seconds` - Event processing overhead

## Dependencies

### Required Crates
```toml
futures = { workspace = true }
metrics = { workspace = true }
num_cpus = { workspace = true }
serde = { workspace = true }
task_executor = { workspace = true }
thiserror = { workspace = true }
tokio = { workspace = true, features = ["sync"] }
tracing = { workspace = true }
```

### Internal Dependencies
- `task_executor::TaskExecutor` - For spawning tasks
- `metrics` - For instrumentation
- Tokio runtime for async execution

## Thread Safety
- All public types implement `Send + Sync` where required
- Uses `Arc<Semaphore>` for permit management
- Channel-based communication between components
- No shared mutable state outside of synchronized primitives

## Performance Characteristics
- **Queue Operations**: O(1) for send/receive
- **Priority Selection**: O(1) biased select
- **Permit Acquisition**: Blocking until available  
- **Memory Usage**: Bounded by queue sizes
- **CPU Usage**: Scales with configured max_workers

## Extensibility Points
- Additional `QueueKind` variants can be added
- Queue sizes configurable per type
- Work item expiry system for time-sensitive tasks
- Pluggable metrics backend via `metrics` crate