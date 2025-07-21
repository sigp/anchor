use std::sync::Arc;

use ssv_types::ClusterId;
use tracing::info;
use types::PublicKeyBytes;

/// Events that can occur in the validator lifecycle (minimal set for state sync fix)
#[derive(Debug, Clone)]
pub enum ValidatorEvent {
    /// A validator has been added to the network
    ValidatorAdded {
        cluster_id: ClusterId,
        validator_pubkey: PublicKeyBytes,
    },
    /// A validator has been removed from the network  
    ValidatorRemoved {
        cluster_id: ClusterId,
        validator_pubkey: PublicKeyBytes,
    },
}

/// Minimal event bus for immediate state synchronization during batch processing
///
/// This addresses the specific issue where validator state changes during batch
/// processing weren't immediately visible to other components, causing sync issues.
#[derive(Debug)]
pub struct EventBus {
    /// Broadcast sender for distributing events
    sender: tokio::sync::broadcast::Sender<ValidatorEvent>,
}

impl EventBus {
    /// Create a new event bus
    pub fn new() -> Self {
        let (sender, _) = tokio::sync::broadcast::channel(1024);
        Self { sender }
    }

    /// Subscribe to validator events
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<ValidatorEvent> {
        self.sender.subscribe()
    }

    /// Emit an event to all subscribers
    pub fn emit(&self, event: ValidatorEvent) {
        match self.sender.send(event) {
            Ok(subscriber_count) => {
                if subscriber_count > 0 {
                    info!(
                        subscribers = subscriber_count,
                        "Event delivered to subscribers"
                    );
                }
            }
            Err(_) => {
                // No subscribers - this is fine during startup
            }
        }
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

/// A thread-safe event bus that can be shared across components
pub type SharedEventBus = Arc<EventBus>;

/// Helper to create a shared event bus
pub fn create_shared_event_bus() -> SharedEventBus {
    Arc::new(EventBus::new())
}

/// Utility to emit events through a shared event bus
pub fn emit_event(event_bus: &SharedEventBus, event: ValidatorEvent) {
    event_bus.emit(event);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_event_bus_basic_functionality() {
        let event_bus = create_shared_event_bus();
        let mut receiver = event_bus.subscribe();

        // Test event emission and reception
        let test_event = ValidatorEvent::ValidatorAdded {
            cluster_id: ClusterId([1u8; 32]),
            validator_pubkey: types::PublicKeyBytes::empty(),
        };

        emit_event(&event_bus, test_event.clone());

        // Receive the event
        let received_event = receiver.recv().await.expect("Should receive event");

        // Verify the event matches
        match (test_event, received_event) {
            (
                ValidatorEvent::ValidatorAdded {
                    cluster_id: c1,
                    validator_pubkey: v1,
                },
                ValidatorEvent::ValidatorAdded {
                    cluster_id: c2,
                    validator_pubkey: v2,
                },
            ) => {
                assert_eq!(c1, c2);
                assert_eq!(v1, v2);
            }
            _ => panic!("Event types don't match"),
        }
    }
}
