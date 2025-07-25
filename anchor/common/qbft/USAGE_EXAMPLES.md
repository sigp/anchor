# QBFT Usage Examples

## Basic Setup

### Creating a QBFT Configuration

```rust
use qbft::{Config, ConfigBuilder, DefaultLeaderFunction};
use ssv_types::OperatorId;
use qbft::qbft_types::InstanceHeight;
use indexmap::IndexSet;
use std::time::Duration;

// Set up committee members
let mut committee = IndexSet::new();
committee.insert(OperatorId::from(1));
committee.insert(OperatorId::from(2));
committee.insert(OperatorId::from(3));
committee.insert(OperatorId::from(4));

// Create configuration with builder pattern
let config = ConfigBuilder::<DefaultLeaderFunction>::new(
    OperatorId::from(1), // This node's ID
    InstanceHeight::from(100), // Instance height
    committee, // Committee members
)
.with_round_time(Duration::from_secs(5))
.with_max_rounds(10)
.with_quorum_size(3) // 3 out of 4 nodes for consensus
.build()
.expect("Valid configuration");
```

### Custom Leader Function

```rust
use qbft::qbft_types::{LeaderFunction, InstanceHeight};
use ssv_types::{OperatorId, Round};
use indexmap::IndexSet;

#[derive(Clone, Debug, Default)]
struct RoundRobinLeader;

impl LeaderFunction for RoundRobinLeader {
    fn leader_function(
        &self,
        operator_id: &OperatorId,
        round: Round,
        instance_height: InstanceHeight,
        committee: &IndexSet<OperatorId>,
    ) -> bool {
        // Simple round-robin based on round number
        let leader_index = (round.get() - 1) % committee.len();
        committee.get_index(leader_index) == Some(operator_id)
    }
}

// Use custom leader function
let config = ConfigBuilder::new_with_leader_fn(
    OperatorId::from(1),
    InstanceHeight::from(100),
    committee,
    RoundRobinLeader,
).build().expect("Valid configuration");
```

## Running Consensus

### Basic Consensus Instance

```rust
use qbft::{Qbft, MessageSender, UnsignedWrappedQbftMessage};
use ssv_types::msgid::MessageId;
use types::Hash256;

// Example consensus data type
#[derive(Clone, Debug)]
struct MyData {
    value: u64,
    hash: Hash256,
}

impl ssv_types::consensus::QbftData for MyData {
    type Hash = Hash256;
    
    fn hash(&self) -> Self::Hash {
        self.hash
    }
    
    fn validate(&self) -> bool {
        // Add validation logic
        self.value > 0
    }
}

impl ssz::Encode for MyData {
    fn as_ssz_bytes(&self) -> Vec<u8> {
        self.value.as_ssz_bytes()
    }
}

impl ssz::Decode for MyData {
    fn from_ssz_bytes(bytes: &[u8]) -> Result<Self, ssz::DecodeError> {
        let value = u64::from_ssz_bytes(bytes)?;
        Ok(MyData {
            value,
            hash: Hash256::from_low_u64_be(value),
        })
    }
}

// Message sender implementation
struct NetworkSender {
    node_id: OperatorId,
}

impl MessageSender for NetworkSender {
    fn send(&mut self, msg: UnsignedWrappedQbftMessage) {
        println!("Node {} sending: {:?}", self.node_id, msg.qbft_message.qbft_message_type);
        // Send to network layer for signing and broadcast
    }
}

// Create consensus instance
let start_data = MyData {
    value: 42,
    hash: Hash256::from_low_u64_be(42),
};

let message_id = MessageId::new(Hash256::random(), 1);
let sender = NetworkSender { node_id: OperatorId::from(1) };

let mut qbft = Qbft::new(
    config,
    start_data,
    message_id,
    sender,
);
```

### Processing Messages

```rust
use qbft::qbft_types::WrappedQbftMessage;
use ssv_types::{
    consensus::{QbftMessage, QbftMessageType},
    message::SignedSSVMessage,
};

// Simulate receiving a proposal message
let proposal_msg = WrappedQbftMessage {
    signed_message: signed_ssv_message, // From network
    qbft_message: QbftMessage {
        qbft_message_type: QbftMessageType::Proposal,
        height: 100,
        round: 1,
        identifier: message_id.into(),
        root: Hash256::from_low_u64_be(42),
        data_round: 0,
        round_change_justification: vec![],
        prepare_justification: vec![],
    },
};

// Process the message
qbft.receive(proposal_msg);

// Check if consensus is complete
if let Some(result) = qbft.completed() {
    match result {
        qbft::qbft_types::Completed::Success(data) => {
            println!("Consensus reached on value: {}", data.value);
        }
        qbft::qbft_types::Completed::TimedOut => {
            println!("Consensus timed out");
        }
    }
}
```

### Handling Round Timeouts

```rust
use std::time::{Duration, Instant};

struct TimedQbft<F, D, S> {
    qbft: Qbft<F, D, S>,
    round_start: Instant,
    round_timeout: Duration,
}

impl<F, D, S> TimedQbft<F, D, S>
where
    F: qbft::qbft_types::LeaderFunction + Clone,
    D: ssv_types::consensus::QbftData<Hash = Hash256>,
    S: MessageSender,
{
    fn new(qbft: Qbft<F, D, S>) -> Self {
        let round_timeout = qbft.config().round_time();
        Self {
            qbft,
            round_start: Instant::now(),
            round_timeout,
        }
    }
    
    fn check_timeout(&mut self) {
        if self.round_start.elapsed() >= self.round_timeout {
            println!("Round {} timed out, advancing", self.qbft.get_round().get());
            self.qbft.end_round();
            self.round_start = Instant::now();
        }
    }
    
    fn receive(&mut self, msg: WrappedQbftMessage) {
        let current_round = self.qbft.get_round();
        self.qbft.receive(msg);
        
        // Reset timer if round changed
        if self.qbft.get_round() != current_round {
            self.round_start = Instant::now();
        }
    }
}
```

## Integration Examples

### With Async Runtime

```rust
use tokio::time::{interval, Duration};
use tokio::sync::mpsc;

struct AsyncQbftNode<F, D> {
    qbft: Option<Qbft<F, D, mpsc::UnboundedSender<UnsignedWrappedQbftMessage>>>,
    message_rx: mpsc::UnboundedReceiver<WrappedQbftMessage>,
    outgoing_tx: mpsc::UnboundedSender<UnsignedWrappedQbftMessage>,
}

impl<F, D> AsyncQbftNode<F, D>
where
    F: qbft::qbft_types::LeaderFunction + Clone,
    D: ssv_types::consensus::QbftData<Hash = Hash256> + Send + 'static,
{
    async fn run(&mut self) {
        let mut round_timer = interval(Duration::from_secs(5));
        
        loop {
            tokio::select! {
                // Handle incoming messages
                Some(msg) = self.message_rx.recv() => {
                    if let Some(ref mut qbft) = self.qbft {
                        qbft.receive(msg);
                        
                        // Check if consensus complete
                        if qbft.completed().is_some() {
                            break;
                        }
                    }
                }
                
                // Handle round timeouts
                _ = round_timer.tick() => {
                    if let Some(ref mut qbft) = self.qbft {
                        qbft.end_round();
                        
                        if qbft.completed().is_some() {
                            break;
                        }
                    }
                }
            }
        }
    }
}
```

### Message Validation Pipeline

```rust
use tracing::{info, warn, error};

struct ValidatingMessageSender<S> {
    inner: S,
    validator: Box<dyn Fn(&UnsignedWrappedQbftMessage) -> bool>,
}

impl<S: MessageSender> MessageSender for ValidatingMessageSender<S> {
    fn send(&mut self, msg: UnsignedWrappedQbftMessage) {
        if (self.validator)(&msg) {
            info!("Sending valid message: {:?}", msg.qbft_message.qbft_message_type);
            self.inner.send(msg);
        } else {
            warn!("Dropping invalid message");
        }
    }
}

// Usage
let validator = Box::new(|msg: &UnsignedWrappedQbftMessage| {
    // Custom validation logic
    msg.qbft_message.height > 0 && msg.qbft_message.round > 0
});

let validating_sender = ValidatingMessageSender {
    inner: NetworkSender { node_id: OperatorId::from(1) },
    validator,
};
```

### Multi-Instance Management

```rust
use std::collections::HashMap;
use qbft::qbft_types::InstanceHeight;

struct QbftManager<F, D, S> {
    instances: HashMap<InstanceHeight, Qbft<F, D, S>>,
    config_template: Config<F>,
}

impl<F, D, S> QbftManager<F, D, S>
where
    F: qbft::qbft_types::LeaderFunction + Clone,
    D: ssv_types::consensus::QbftData<Hash = Hash256> + Clone,
    S: MessageSender + Clone,
{
    fn start_instance(&mut self, height: InstanceHeight, data: D, sender: S) {
        let mut config = self.config_template.clone();
        // Update config for specific instance...
        
        let message_id = MessageId::new(Hash256::random(), height.into());
        let qbft = Qbft::new(config, data, message_id, sender);
        self.instances.insert(height, qbft);
    }
    
    fn process_message(&mut self, height: InstanceHeight, msg: WrappedQbftMessage) {
        if let Some(instance) = self.instances.get_mut(&height) {
            instance.receive(msg);
        } else {
            warn!("Received message for unknown instance {}", *height);
        }
    }
    
    fn cleanup_completed(&mut self) -> Vec<(InstanceHeight, qbft::qbft_types::Completed<D>)> {
        let mut completed = Vec::new();
        self.instances.retain(|&height, instance| {
            if let Some(result) = instance.completed() {
                completed.push((height, result));
                false // Remove completed instance
            } else {
                true // Keep running instance
            }
        });
        completed
    }
}
```

## Error Handling

### Configuration Validation

```rust
use qbft::error::ConfigBuilderError;

fn create_safe_config(
    operator_id: OperatorId,
    committee: IndexSet<OperatorId>,
) -> Result<Config<DefaultLeaderFunction>, ConfigBuilderError> {
    // Validate inputs before building
    if committee.is_empty() {
        return Err(ConfigBuilderError::NoParticipants);
    }
    
    if !committee.contains(&operator_id) {
        return Err(ConfigBuilderError::OperatorNotParticipant);
    }
    
    let quorum_size = (committee.len() * 2 / 3) + 1;
    
    ConfigBuilder::new(operator_id, InstanceHeight::from(1), committee)
        .with_quorum_size(quorum_size)
        .build()
}
```

### Message Processing with Error Recovery

```rust
struct RobustQbft<F, D, S> {
    qbft: Qbft<F, D, S>,
    failed_messages: Vec<WrappedQbftMessage>,
}

impl<F, D, S> RobustQbft<F, D, S>
where
    F: qbft::qbft_types::LeaderFunction + Clone,
    D: ssv_types::consensus::QbftData<Hash = Hash256>,
    S: MessageSender,
{
    fn safe_receive(&mut self, msg: WrappedQbftMessage) {
        // Store message state before processing
        let prev_round = self.qbft.get_round();
        let prev_completed = self.qbft.completed().is_some();
        
        // Process message
        self.qbft.receive(msg.clone());
        
        // Check for unexpected state changes
        if self.qbft.completed().is_some() && !prev_completed {
            info!("Consensus completed successfully");
        } else if self.qbft.get_round() != prev_round {
            info!("Advanced to round {}", self.qbft.get_round().get());
            
            // Retry failed messages from previous rounds
            let failed = std::mem::take(&mut self.failed_messages);
            for old_msg in failed {
                self.qbft.receive(old_msg);
            }
        }
    }
}
```