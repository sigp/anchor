//! Topic scoring configuration for SSV gossipsub topics.
//!
//! This module provides dynamic topic scoring parameters that adapt to network conditions,
//! validator counts, and committee structures following SSV specifications.

use std::time::Duration;

use gossipsub::TopicScoreParams;
use ssv_types::CommitteeInfo;
use subnet_tracker::SubnetId;
use tracing::{debug, warn};
use types::{ChainSpec, EthSpec};

use crate::scoring::{
    decay_threshold,
    message_rate::calculate_message_rate_for_topic,
    peer_score_config::{GRAYLIST_THRESHOLD, calculate_score_decay_factor, decay_convergence},
};

// SSV Network topology constants (matching Go implementation)
const GOSSIPSUB_D: usize = 8;
const TOTAL_TOPICS_WEIGHT: f64 = 4.0;

// P1: Time in Mesh parameters
const MAX_TIME_IN_MESH_SCORE: f64 = 10.0;
const TIME_IN_MESH_QUANTUM: Duration = Duration::from_secs(12); // seconds
const TIME_IN_MESH_QUANTUM_CAP: Duration = Duration::from_secs(3600); // seconds (1 hour)

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
    pub time_in_mesh_quantum: Duration,
    pub time_in_mesh_quantum_cap: Duration,

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

impl TopicScoringOptions {
    /// Create new options with the given network parameters
    pub fn new<E: EthSpec>(
        active_validators: u64,
        subnets: usize,
        committees: &[CommitteeInfo],
        chain_spec: &ChainSpec,
    ) -> Self {
        let slot_duration = Duration::from_secs(chain_spec.seconds_per_slot);
        let one_epoch_duration = E::slots_per_epoch() as u32 * slot_duration;

        let network = NetworkConfig {
            active_validators,
            subnets,
            one_epoch_duration,
            total_topics_weight: TOTAL_TOPICS_WEIGHT,
        };

        let topic = TopicConfig {
            mesh_delivery_activation_time: one_epoch_duration * 3,
            topic_weight: network.total_topics_weight / subnets as f64, /* Set topic weight with
                                                                         * equal weights across
                                                                         * all subnets */
            expected_msg_rate: calculate_message_rate_for_topic::<E>(committees, chain_spec),
            ..Default::default()
        };

        Self { network, topic }
    }

    /// Calculate the maximum score attainable by a peer
    pub fn max_score(&self) -> f64 {
        (self.topic.max_time_in_mesh_score + self.topic.max_first_delivery_score)
            * self.network.total_topics_weight
    }

    /// Generate gossipsub TopicScoreParams from this configuration
    pub fn to_topic_score_params(&self) -> Result<TopicScoreParams, String> {
        let decay_interval = self.network.one_epoch_duration;
        let expected_messages_per_decay_interval =
            self.topic.expected_msg_rate * decay_interval.as_secs_f64();

        // P1: Time in Mesh
        let time_in_mesh_cap = self
            .topic
            .time_in_mesh_quantum_cap
            .div_duration_f64(self.topic.time_in_mesh_quantum);
        let time_in_mesh_weight = self.topic.max_time_in_mesh_score / time_in_mesh_cap;

        // P2: First Message Deliveries
        let first_delivery_decay_duration =
            self.network.one_epoch_duration * self.topic.first_delivery_decay_epochs;
        let first_message_deliveries_decay =
            calculate_score_decay_factor(first_delivery_decay_duration, decay_interval);

        let first_message_deliveries_cap = if expected_messages_per_decay_interval > 0.0 {
            decay_convergence(
                first_message_deliveries_decay,
                2.0 * expected_messages_per_decay_interval / self.topic.d as f64,
            )
            .map_err(|e| {
                format!(
                    "Could not calculate decay convergence for first message delivery cap: {e}",
                )
            })?
        } else {
            1.0
        };

        let first_message_deliveries_weight =
            self.topic.max_first_delivery_score / first_message_deliveries_cap;

        // P3: Mesh Message Deliveries
        let mesh_delivery_decay_duration =
            self.network.one_epoch_duration * self.topic.mesh_delivery_decay_epochs;
        let mesh_message_deliveries_decay =
            calculate_score_decay_factor(mesh_delivery_decay_duration, decay_interval);

        let mesh_message_deliveries_threshold = if expected_messages_per_decay_interval > 0.0 {
            decay_threshold(
                mesh_message_deliveries_decay,
                expected_messages_per_decay_interval * self.topic.mesh_delivery_dampening_factor,
            )
            .map_err(|e| {
                format!("Could not calculate threshold for mesh message deliveries: {e}")
            })?
        } else {
            1.0
        };

        // Mesh scoring is disabled in SSV
        let mesh_message_deliveries_weight = 0.0;

        let mesh_message_deliveries_cap =
            mesh_message_deliveries_threshold * self.topic.mesh_delivery_cap_factor;

        // P4: Invalid Message Deliveries
        let invalid_decay_duration =
            self.network.one_epoch_duration * self.topic.invalid_message_decay_epochs;
        let invalid_message_deliveries_decay =
            calculate_score_decay_factor(invalid_decay_duration, decay_interval);

        let invalid_message_deliveries_weight = GRAYLIST_THRESHOLD
            / (self.topic.topic_weight
                * self.topic.max_invalid_messages_allowed as f64
                * self.topic.max_invalid_messages_allowed as f64);

        let mut params = TopicScoreParams {
            topic_weight: self.topic.topic_weight,

            // P1: Time in Mesh
            time_in_mesh_quantum: self.topic.time_in_mesh_quantum,
            time_in_mesh_cap,
            time_in_mesh_weight,

            // P2: First Message Deliveries
            first_message_deliveries_decay,
            first_message_deliveries_cap,
            first_message_deliveries_weight,

            // P3: Mesh Message Deliveries
            mesh_message_deliveries_decay,
            mesh_message_deliveries_threshold,
            mesh_message_deliveries_weight,
            mesh_message_deliveries_cap,
            mesh_message_deliveries_activation: self.topic.mesh_delivery_activation_time,
            mesh_message_deliveries_window: Duration::from_secs(2),

            // P3b: Mesh Failure Penalty
            mesh_failure_penalty_decay: mesh_message_deliveries_decay,
            mesh_failure_penalty_weight: mesh_message_deliveries_weight,

            // P4: Invalid Message Deliveries
            invalid_message_deliveries_decay,
            invalid_message_deliveries_weight,
        };

        // Sanitize parameters to handle NaN/Inf values
        let sanitized_count = Self::sanitize_topic_params(&mut params);
        if sanitized_count > 0 {
            warn!(
                "Sanitized {sanitized_count} invalid topic scoring parameters (NaN/Inf values replaced with defaults)",
            );
        }

        Ok(params)
    }

    /// Sanitize TopicScoreParams by replacing NaN/Inf values with defaults
    ///
    /// Returns the number of parameters that were sanitized, which can be used
    /// for logging or monitoring purposes.
    fn sanitize_topic_params(params: &mut TopicScoreParams) -> usize {
        const DEFAULT_DECAY: f64 = 0.001;
        const DEFAULT_WEIGHT: f64 = 0.0;
        const DEFAULT_CAP: f64 = 1.0;
        const DEFAULT_THRESHOLD: f64 = 1.0;
        const DEFAULT_INVALID_WEIGHT: f64 = -0.1;

        let mut sanitized_count = 0;

        let mut sanitize_param = |value: &mut f64, default: f64| {
            if value.is_nan() || value.is_infinite() {
                *value = default;
                sanitized_count += 1;
            }
        };

        // P1: Time in Mesh
        sanitize_param(&mut params.time_in_mesh_cap, DEFAULT_CAP);
        sanitize_param(&mut params.time_in_mesh_weight, DEFAULT_WEIGHT);

        // P2: First Message Deliveries
        sanitize_param(&mut params.first_message_deliveries_decay, DEFAULT_DECAY);
        sanitize_param(&mut params.first_message_deliveries_cap, DEFAULT_CAP);
        sanitize_param(&mut params.first_message_deliveries_weight, DEFAULT_WEIGHT);

        // P3: Mesh Message Deliveries
        sanitize_param(&mut params.mesh_message_deliveries_decay, DEFAULT_DECAY);
        sanitize_param(
            &mut params.mesh_message_deliveries_threshold,
            DEFAULT_THRESHOLD,
        );
        sanitize_param(&mut params.mesh_message_deliveries_weight, DEFAULT_WEIGHT);
        sanitize_param(&mut params.mesh_message_deliveries_cap, DEFAULT_CAP);

        // P3b: Mesh Failure Penalty
        sanitize_param(&mut params.mesh_failure_penalty_decay, DEFAULT_DECAY);
        sanitize_param(&mut params.mesh_failure_penalty_weight, DEFAULT_WEIGHT);

        // P4: Invalid Message Deliveries
        sanitize_param(&mut params.invalid_message_deliveries_decay, DEFAULT_DECAY);
        sanitize_param(
            &mut params.invalid_message_deliveries_weight,
            DEFAULT_INVALID_WEIGHT,
        );

        sanitized_count
    }
}

/// Generate topic score parameters for a specific subnet
pub fn topic_score_params_for_subnet<E: EthSpec>(
    subnet: SubnetId,
    validator_count: u64,
    subnet_count: u64,
    committees: &[CommitteeInfo],
    chain_spec: &ChainSpec,
) -> TopicScoreParams {
    // Create options using committee-based calculation with the new message rate function
    let opts = TopicScoringOptions::new::<E>(
        validator_count,
        subnet_count as usize,
        committees,
        chain_spec,
    );

    // Generate and return parameters
    match opts.to_topic_score_params() {
        Ok(params) => {
            debug!(
                subnet = *subnet,
                validator_count = validator_count,
                committee_count = committees.len(),
                expected_rate = opts.topic.expected_msg_rate,
                topic_weight = opts.topic.topic_weight,
                "Generated topic score parameters for subnet"
            );
            params
        }
        Err(e) => {
            warn!(
                subnet = *subnet,
                error = %e,
                "Failed to generate topic score parameters, using defaults"
            );
            // Return safe default parameters
            TopicScoreParams::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use gossipsub::TopicScoreParams;

    use super::*;

    #[test]
    fn test_sanitize_topic_params_with_valid_values() {
        let mut params = TopicScoreParams {
            time_in_mesh_weight: 1.0,
            first_message_deliveries_cap: 2.0,
            mesh_message_deliveries_threshold: 3.0,
            ..Default::default()
        };

        let sanitized_count = TopicScoringOptions::sanitize_topic_params(&mut params);

        assert_eq!(sanitized_count, 0, "No parameters should be sanitized");
        assert_eq!(params.time_in_mesh_weight, 1.0);
        assert_eq!(params.first_message_deliveries_cap, 2.0);
        assert_eq!(params.mesh_message_deliveries_threshold, 3.0);
    }

    #[test]
    fn test_sanitize_topic_params_with_nan_values() {
        let mut params = TopicScoreParams {
            time_in_mesh_weight: f64::NAN,
            first_message_deliveries_cap: f64::NAN,
            mesh_message_deliveries_threshold: f64::NAN,
            ..Default::default()
        };

        let sanitized_count = TopicScoringOptions::sanitize_topic_params(&mut params);

        assert_eq!(sanitized_count, 3, "Three parameters should be sanitized");
        assert!(!params.time_in_mesh_weight.is_nan());
        assert!(!params.first_message_deliveries_cap.is_nan());
        assert!(!params.mesh_message_deliveries_threshold.is_nan());

        // Check that defaults are applied correctly
        assert_eq!(params.time_in_mesh_weight, 0.0);
        assert_eq!(params.first_message_deliveries_cap, 1.0);
        assert_eq!(params.mesh_message_deliveries_threshold, 1.0);
    }

    #[test]
    fn test_sanitize_topic_params_with_infinite_values() {
        let mut params = TopicScoreParams {
            time_in_mesh_cap: f64::INFINITY,
            first_message_deliveries_decay: f64::NEG_INFINITY,
            invalid_message_deliveries_weight: f64::INFINITY,
            ..Default::default()
        };

        let sanitized_count = TopicScoringOptions::sanitize_topic_params(&mut params);

        assert_eq!(sanitized_count, 3, "Three parameters should be sanitized");
        assert!(!params.time_in_mesh_cap.is_infinite());
        assert!(!params.first_message_deliveries_decay.is_infinite());
        assert!(!params.invalid_message_deliveries_weight.is_infinite());

        // Check that defaults are applied correctly
        assert_eq!(params.time_in_mesh_cap, 1.0);
        assert_eq!(params.first_message_deliveries_decay, 0.001);
        assert_eq!(params.invalid_message_deliveries_weight, -0.1);
    }

    #[test]
    fn test_sanitize_topic_params_mixed_valid_invalid() {
        let mut params = TopicScoreParams {
            time_in_mesh_weight: 5.0,                     // Valid
            first_message_deliveries_cap: f64::NAN,       // Invalid
            mesh_message_deliveries_threshold: 10.0,      // Valid
            mesh_message_deliveries_decay: f64::INFINITY, // Invalid
            ..Default::default()
        };

        let sanitized_count = TopicScoringOptions::sanitize_topic_params(&mut params);

        assert_eq!(sanitized_count, 2, "Two parameters should be sanitized");
        assert_eq!(params.time_in_mesh_weight, 5.0); // Unchanged
        assert_eq!(params.first_message_deliveries_cap, 1.0); // Sanitized
        assert_eq!(params.mesh_message_deliveries_threshold, 10.0); // Unchanged
        assert_eq!(params.mesh_message_deliveries_decay, 0.001); // Sanitized
    }

    #[test]
    fn test_sanitize_topic_params_all_parameters() {
        // Test that all parameters are handled correctly
        let mut params = TopicScoreParams {
            time_in_mesh_cap: f64::NAN,
            time_in_mesh_weight: f64::NAN,
            first_message_deliveries_decay: f64::NAN,
            first_message_deliveries_cap: f64::NAN,
            first_message_deliveries_weight: f64::NAN,
            mesh_message_deliveries_decay: f64::NAN,
            mesh_message_deliveries_threshold: f64::NAN,
            mesh_message_deliveries_weight: f64::NAN,
            mesh_message_deliveries_cap: f64::NAN,
            mesh_failure_penalty_decay: f64::NAN,
            mesh_failure_penalty_weight: f64::NAN,
            invalid_message_deliveries_decay: f64::NAN,
            invalid_message_deliveries_weight: f64::NAN,
            ..Default::default()
        };

        let sanitized_count = TopicScoringOptions::sanitize_topic_params(&mut params);

        assert_eq!(sanitized_count, 13, "All 13 parameters should be sanitized");

        // Verify no NaN values remain
        assert!(!params.time_in_mesh_cap.is_nan());
        assert!(!params.time_in_mesh_weight.is_nan());
        assert!(!params.first_message_deliveries_decay.is_nan());
        assert!(!params.first_message_deliveries_cap.is_nan());
        assert!(!params.first_message_deliveries_weight.is_nan());
        assert!(!params.mesh_message_deliveries_decay.is_nan());
        assert!(!params.mesh_message_deliveries_threshold.is_nan());
        assert!(!params.mesh_message_deliveries_weight.is_nan());
        assert!(!params.mesh_message_deliveries_cap.is_nan());
        assert!(!params.mesh_failure_penalty_decay.is_nan());
        assert!(!params.mesh_failure_penalty_weight.is_nan());
        assert!(!params.invalid_message_deliveries_decay.is_nan());
        assert!(!params.invalid_message_deliveries_weight.is_nan());
    }
}
