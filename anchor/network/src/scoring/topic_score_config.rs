//! Topic scoring configuration for SSV gossipsub topics.
//!
//! This module provides dynamic topic scoring parameters that adapt to network conditions,
////! validator counts, and committee structures following SSV specifications.

use std::time::Duration;

// SSV Network topology constants (matching Go implementation)
const GOSSIPSUB_D: usize = 8;
const TOTAL_TOPICS_WEIGHT: f64 = 4.0;

// P1: Time in Mesh parameters
const MAX_TIME_IN_MESH_SCORE: f64 = 10.0;
const TIME_IN_MESH_QUANTUM: u64 = 12; // seconds
const TIME_IN_MESH_QUANTUM_CAP: u64 = 3600; // seconds (1 hour)

// P2: First Message Deliveries parameters
const FIRST_DELIVERY_DECAY_EPOCHS: u32 = 4;
const MAX_FIRST_DELIVERY_SCORE: f64 = 80.0;

// P3: Mesh Message Deliveries parameters
const MESH_DELIVERY_DECAY_EPOCHS: u32 = 16;
const MESH_DELIVERY_DAMPENING_FACTOR: f64 = 1.0 / 50.0;
const MESH_DELIVERY_CAP_FACTOR: f64 = 16.0;

// P4: Invalid Message Deliveries parameters
const INVALID_MESSAGE_DECAY_EPOCHS: u32 = 100;
const MAX_INVALID_MESSAGES_ALLOWED: usize = 20;

/// Network-wide configuration options for topic scoring
#[derive(Debug, Clone)]
pub struct NetworkConfig {
    /// Total number of active validators in the network
    pub active_validators: u64,
    /// Number of subnets in the network
    pub subnets: usize,
    /// Duration of one epoch
    pub one_epoch_duration: Duration,
    /// Total weight allocated across all topics
    pub total_topics_weight: f64,
}

/// Topic-specific configuration options
#[derive(Debug, Clone)]
pub struct TopicConfig {
    /// Gossip degree (D parameter)
    pub d: usize,
    /// Expected message rate for this topic (messages per second)
    pub expected_msg_rate: f64,
    /// Weight assigned to this specific topic
    pub topic_weight: f64,

    // P1: Time in Mesh
    pub max_time_in_mesh_score: f64,
    pub time_in_mesh_quantum: u64,
    pub time_in_mesh_quantum_cap: u64,

    // P2: First Message Deliveries
    pub first_delivery_decay_epochs: u32,
    pub max_first_delivery_score: f64,

    // P3: Mesh Message Deliveries
    pub mesh_delivery_decay_epochs: u32,
    pub mesh_delivery_dampening_factor: f64,
    pub mesh_delivery_cap_factor: f64,
    pub mesh_delivery_activation_time: Duration,

    // P4: Invalid Message Deliveries
    pub invalid_message_decay_epochs: u32,
    pub max_invalid_messages_allowed: usize,
}

/// Complete configuration for topic score calculation
#[derive(Debug, Clone)]
pub struct TopicScoringOptions {
    pub network: NetworkConfig,
    pub topic: TopicConfig,
}

impl Default for TopicConfig {
    fn default() -> Self {
        Self {
            d: GOSSIPSUB_D,
            expected_msg_rate: 0.0,
            topic_weight: 0.0,
            max_time_in_mesh_score: MAX_TIME_IN_MESH_SCORE,
            time_in_mesh_quantum: TIME_IN_MESH_QUANTUM,
            time_in_mesh_quantum_cap: TIME_IN_MESH_QUANTUM_CAP,
            first_delivery_decay_epochs: FIRST_DELIVERY_DECAY_EPOCHS,
            max_first_delivery_score: MAX_FIRST_DELIVERY_SCORE,
            mesh_delivery_decay_epochs: MESH_DELIVERY_DECAY_EPOCHS,
            mesh_delivery_dampening_factor: MESH_DELIVERY_DAMPENING_FACTOR,
            mesh_delivery_cap_factor: MESH_DELIVERY_CAP_FACTOR,
            mesh_delivery_activation_time: Duration::ZERO,
            invalid_message_decay_epochs: INVALID_MESSAGE_DECAY_EPOCHS,
            max_invalid_messages_allowed: MAX_INVALID_MESSAGES_ALLOWED,
        }
    }
}
