use std::sync::Arc;

use ssv_types::{Cluster, ClusterId, ValidatorMetadata};
use tokio::sync::mpsc::{self, error::TrySendError};
use tracing::{debug, warn};
use types::{Address, PublicKeyBytes, SecretKey};

/// Default capacity for the validator event channel.
///
/// This is a blocking MPSC channel - when full, the event processor will block
/// until the validator store processes events. This ensures no events are lost.
///
/// The capacity is chosen to handle reasonable bursts of validator lifecycle events:
/// - Validator add/remove events are relatively infrequent in normal operation
/// - During sync, events may arrive in larger batches from historical blocks
/// - Each event contains full validator data (~1KB: Cluster + ValidatorMetadata + optional
///   SecretKey)
/// - Total memory usage: ~2MB for full buffer
///
/// If the validator store falls behind, the event processor will block, providing
/// natural backpressure to prevent unbounded memory growth.
const DEFAULT_EVENT_CHANNEL_CAPACITY: usize = 2048;

/// Events that can occur in the validator lifecycle
///
/// Each event contains all the data needed by the validator store to update its state
/// without requiring database access.
#[derive(Clone)]
pub enum ValidatorEvent {
    /// A validator has been added to the network
    ValidatorAdded {
        validator_pubkey: PublicKeyBytes,
        cluster: Box<Cluster>,
        metadata: ValidatorMetadata,
        decrypted_key_share: Option<SecretKey>,
    },
    /// A validator has been removed from the network  
    ValidatorRemoved {
        validator_pubkey: PublicKeyBytes,
        cluster_id: ClusterId,
    },
    /// Fee recipient has been updated for one or more clusters
    FeeRecipientUpdated {
        cluster_ids: Vec<ClusterId>,
        new_fee_recipient: Address,
    },
    /// A cluster has been liquidated
    ClusterLiquidated { cluster_id: ClusterId },
    /// A cluster has been reactivated
    ClusterReactivated { cluster_id: ClusterId },
}

impl std::fmt::Debug for ValidatorEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ValidatorEvent::ValidatorAdded {
                validator_pubkey,
                cluster,
                metadata,
                decrypted_key_share,
            } => f
                .debug_struct("ValidatorAdded")
                .field("validator_pubkey", validator_pubkey)
                .field("cluster", cluster)
                .field("metadata", metadata)
                .field("has_key_share", &decrypted_key_share.is_some())
                .finish(),
            ValidatorEvent::ValidatorRemoved {
                validator_pubkey,
                cluster_id,
            } => f
                .debug_struct("ValidatorRemoved")
                .field("validator_pubkey", validator_pubkey)
                .field("cluster_id", cluster_id)
                .finish(),
            ValidatorEvent::FeeRecipientUpdated {
                cluster_ids,
                new_fee_recipient,
            } => f
                .debug_struct("FeeRecipientUpdated")
                .field("cluster_ids", cluster_ids)
                .field("new_fee_recipient", new_fee_recipient)
                .finish(),
            ValidatorEvent::ClusterLiquidated { cluster_id } => f
                .debug_struct("ClusterLiquidated")
                .field("cluster_id", cluster_id)
                .finish(),
            ValidatorEvent::ClusterReactivated { cluster_id } => f
                .debug_struct("ClusterReactivated")
                .field("cluster_id", cluster_id)
                .finish(),
        }
    }
}

/// Event bus for reliable delivery of validator lifecycle events
///
/// This uses a blocking MPSC channel to ensure no events are ever lost.
/// When the channel is full, the event processor will block until the
/// validator store processes more events.
#[derive(Debug)]
pub struct EventBus {
    /// Sender for distributing events
    sender: mpsc::Sender<ValidatorEvent>,
}

impl EventBus {
    /// Create a new event bus with default capacity
    pub fn new() -> (Self, mpsc::Receiver<ValidatorEvent>) {
        Self::with_capacity(DEFAULT_EVENT_CHANNEL_CAPACITY)
    }

    /// Create a new event bus with custom capacity
    ///
    /// # Arguments
    /// * `capacity` - Maximum number of events to buffer before blocking the sender
    ///
    /// # Returns
    /// A tuple of (EventBus, Receiver) where the receiver should be used
    /// by the validator store to process events
    pub fn with_capacity(capacity: usize) -> (Self, mpsc::Receiver<ValidatorEvent>) {
        let (sender, receiver) = mpsc::channel(capacity);
        (Self { sender }, receiver)
    }

    /// Emit an event - will block if channel is full
    ///
    /// This ensures no events are ever lost, which is critical for maintaining
    /// validator store consistency.
    pub async fn emit(&self, event: ValidatorEvent) {
        match self.sender.send(event).await {
            Ok(()) => {
                // Event successfully sent
                debug!("Validator event emitted successfully");
            }
            Err(_) => {
                // Receiver was dropped - this is only expected during shutdown
                warn!("Failed to send validator event: receiver dropped");
            }
        }
    }

    /// Try to emit an event without blocking
    ///
    /// Returns an error if the channel is full. Use this only when you can
    /// handle the error appropriately or retry.
    pub fn try_emit(&self, event: ValidatorEvent) -> Result<(), TrySendError<()>> {
        self.sender.try_send(event).map_err(|e| match e {
            TrySendError::Full(_) => TrySendError::Full(()),
            TrySendError::Closed(_) => TrySendError::Closed(()),
        })
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new().0
    }
}

/// A thread-safe event bus that can be shared across components
pub type SharedEventBus = Arc<EventBus>;

/// Helper to create a shared event bus and receiver
///
/// # Returns
/// A tuple of (SharedEventBus, Receiver) where the receiver should be used
/// by the validator store to process events
pub fn create_shared_event_bus() -> (SharedEventBus, mpsc::Receiver<ValidatorEvent>) {
    let (event_bus, receiver) = EventBus::new();
    (Arc::new(event_bus), receiver)
}

/// Utility to emit events through a shared event bus (async version)
pub async fn emit_event(event_bus: &SharedEventBus, event: ValidatorEvent) {
    event_bus.emit(event).await;
}

/// Utility to emit events through a shared event bus (non-blocking version)
///
/// Use this only when you can handle the TrySendError appropriately.
pub fn try_emit_event(
    event_bus: &SharedEventBus,
    event: ValidatorEvent,
) -> Result<(), TrySendError<()>> {
    event_bus.try_emit(event)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_event_bus_basic_functionality() {
        let (event_bus, mut receiver) = create_shared_event_bus();

        // Test event emission and reception
        let test_event = ValidatorEvent::ValidatorAdded {
            validator_pubkey: PublicKeyBytes::empty(),
            cluster: Box::new(Cluster {
                cluster_id: ClusterId([1u8; 32]),
                owner: Default::default(),
                fee_recipient: Default::default(),
                liquidated: false,
                cluster_members: Default::default(),
            }),
            metadata: ValidatorMetadata {
                public_key: PublicKeyBytes::empty(),
                cluster_id: ClusterId([1u8; 32]),
                index: None,
                graffiti: Default::default(),
            },
            decrypted_key_share: None,
        };

        emit_event(&event_bus, test_event.clone()).await;

        // Receive the event
        let received_event = receiver.recv().await.expect("Should receive event");

        // Verify the event matches
        match (test_event, received_event) {
            (
                ValidatorEvent::ValidatorAdded {
                    validator_pubkey: v1,
                    cluster: c1,
                    metadata: m1,
                    ..
                },
                ValidatorEvent::ValidatorAdded {
                    validator_pubkey: v2,
                    cluster: c2,
                    metadata: m2,
                    ..
                },
            ) => {
                assert_eq!(v1, v2);
                assert_eq!(c1.cluster_id, c2.cluster_id);
                assert_eq!(m1.public_key, m2.public_key);
            }
            _ => panic!("Event types don't match"),
        }
    }

    #[tokio::test]
    async fn test_blocking_behavior() {
        // Create a very small capacity bus to test blocking
        let (sender, receiver) = EventBus::with_capacity(1);
        let sender = Arc::new(sender);

        // Fill the channel
        sender
            .try_emit(ValidatorEvent::ValidatorRemoved {
                validator_pubkey: PublicKeyBytes::empty(),
                cluster_id: ClusterId([1u8; 32]),
            })
            .expect("First send should succeed");

        // Second send should fail with try_emit
        assert!(
            sender
                .try_emit(ValidatorEvent::ValidatorRemoved {
                    validator_pubkey: PublicKeyBytes::empty(),
                    cluster_id: ClusterId([2u8; 32]),
                })
                .is_err()
        );

        // Clean up
        drop(receiver);
    }

    #[tokio::test]
    async fn test_fee_recipient_updated_with_multiple_clusters() {
        let (event_bus, mut receiver) = create_shared_event_bus();

        // Test fee recipient update with multiple cluster IDs
        let cluster_ids = vec![
            ClusterId([1u8; 32]),
            ClusterId([2u8; 32]),
            ClusterId([3u8; 32]),
        ];
        let new_fee_recipient = Address::from([0x42u8; 20]);

        let test_event = ValidatorEvent::FeeRecipientUpdated {
            cluster_ids: cluster_ids.clone(),
            new_fee_recipient,
        };

        emit_event(&event_bus, test_event.clone()).await;

        // Receive the event
        let received_event = receiver.recv().await.expect("Should receive event");

        // Verify the event matches
        match received_event {
            ValidatorEvent::FeeRecipientUpdated {
                cluster_ids: received_cluster_ids,
                new_fee_recipient: received_fee_recipient,
            } => {
                assert_eq!(cluster_ids, received_cluster_ids);
                assert_eq!(new_fee_recipient, received_fee_recipient);
            }
            _ => panic!("Expected FeeRecipientUpdated event"),
        }
    }
}
