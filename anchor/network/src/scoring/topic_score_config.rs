//! Topic scoring configuration for SSV gossipsub topics.
//!
//! This module provides dynamic topic scoring parameters that adapt to network conditions,
////! validator counts, and committee structures following SSV specifications.

use std::time::Duration;

use gossipsub::TopicScoreParams;
use ssv_types::CommitteeInfo;
use subnet_tracker::SubnetId;
use tracing::{debug, warn};

use crate::scoring::{
    calculate_score_decay_factor, decay_convergence, decay_threshold,
    message_rate::calculate_message_rate_for_topic, peer_score_config::GRAYLIST_THRESHOLD,
};

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
const MESH_SCORING_ENABLED: bool = false; // Disabled in SSV

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
            mesh_delivery_activation_time: Duration::ZERO, // Will be set to 3 * one_epoch_duration
            invalid_message_decay_epochs: INVALID_MESSAGE_DECAY_EPOCHS,
            max_invalid_messages_allowed: MAX_INVALID_MESSAGES_ALLOWED,
        }
    }
}

impl TopicScoringOptions {
    /// Create new options with the given network parameters
    pub fn new(
        active_validators: u64,
        subnets: usize,
        committees: &[CommitteeInfo],
        one_epoch_duration: Duration,
    ) -> Self {
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
            expected_msg_rate: calculate_message_rate_for_topic(committees),
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
        let time_in_mesh_cap =
            self.topic.time_in_mesh_quantum_cap as f64 / self.topic.time_in_mesh_quantum as f64;
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
                    "Could not calculate decay convergence for first message delivery cap: {}",
                    e
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
                format!(
                    "Could not calculate threshold for mesh message deliveries: {}",
                    e
                )
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
            time_in_mesh_quantum: Duration::from_secs(self.topic.time_in_mesh_quantum),
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
        Self::sanitize_topic_params(&mut params);

        Ok(params)
    }

    /// Sanitize TopicScoreParams by replacing NaN/Inf values with defaults
    fn sanitize_topic_params(params: &mut TopicScoreParams) {
        const DEFAULT_DECAY: f64 = 0.001;
        const DEFAULT_WEIGHT: f64 = 0.0;
        const DEFAULT_CAP: f64 = 1.0;
        const DEFAULT_THRESHOLD: f64 = 1.0;
        const DEFAULT_INVALID_WEIGHT: f64 = -0.1;

        fn sanitize_parameter(value: f64, default: f64) -> f64 {
            if value.is_nan() || value.is_infinite() {
                default
            } else {
                value
            }
        }

        // P1
        params.time_in_mesh_cap = sanitize_parameter(params.time_in_mesh_cap, DEFAULT_CAP);
        params.time_in_mesh_weight = sanitize_parameter(params.time_in_mesh_weight, DEFAULT_WEIGHT);

        // P2
        params.first_message_deliveries_decay =
            sanitize_parameter(params.first_message_deliveries_decay, DEFAULT_DECAY);
        params.first_message_deliveries_cap =
            sanitize_parameter(params.first_message_deliveries_cap, DEFAULT_CAP);
        params.first_message_deliveries_weight =
            sanitize_parameter(params.first_message_deliveries_weight, DEFAULT_WEIGHT);

        // P3
        params.mesh_message_deliveries_decay =
            sanitize_parameter(params.mesh_message_deliveries_decay, DEFAULT_DECAY);
        params.mesh_message_deliveries_threshold =
            sanitize_parameter(params.mesh_message_deliveries_threshold, DEFAULT_THRESHOLD);
        params.mesh_message_deliveries_weight =
            sanitize_parameter(params.mesh_message_deliveries_weight, DEFAULT_WEIGHT);
        params.mesh_message_deliveries_cap =
            sanitize_parameter(params.mesh_message_deliveries_cap, DEFAULT_CAP);

        // P3b
        params.mesh_failure_penalty_decay =
            sanitize_parameter(params.mesh_failure_penalty_decay, DEFAULT_DECAY);
        params.mesh_failure_penalty_weight =
            sanitize_parameter(params.mesh_failure_penalty_weight, DEFAULT_WEIGHT);

        // P4
        params.invalid_message_deliveries_decay =
            sanitize_parameter(params.invalid_message_deliveries_decay, DEFAULT_DECAY);
        params.invalid_message_deliveries_weight = sanitize_parameter(
            params.invalid_message_deliveries_weight,
            DEFAULT_INVALID_WEIGHT,
        );
    }
}

/// Generate topic score parameters for a specific subnet
pub fn topic_score_params_for_subnet(
    one_epoch_duration: Duration,
    subnet: SubnetId,
    validator_count: u64,
    subnet_count: u64,
    committees: &[CommitteeInfo],
) -> TopicScoreParams {
    // Create options using committee-based calculation with the new message rate function
    let opts = TopicScoringOptions::new(
        validator_count,
        subnet_count as usize,
        committees,
        one_epoch_duration,
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
    use ssv_types::{IndexSet, OperatorId, ValidatorIndex};

    use super::*;

    fn mock_committee() -> Vec<CommitteeInfo> {
        // Create a mock committee for testing
        let committees = vec![CommitteeInfo {
            committee_members: IndexSet::from([
                OperatorId(0),
                OperatorId(1),
                OperatorId(2),
                OperatorId(3),
            ]),
            validator_indices: vec![
                ValidatorIndex(0),
                ValidatorIndex(1),
                ValidatorIndex(2),
                ValidatorIndex(3),
            ],
        }];
        committees
    }

    #[test]
    fn test_topic_scoring_options_creation() {
        let opts =
            TopicScoringOptions::new(100_000, 128, &mock_committee(), Duration::from_secs(384));

        assert_eq!(opts.network.active_validators, 100_000);
        assert_eq!(opts.network.subnets, 128);
        assert_eq!(opts.network.total_topics_weight, TOTAL_TOPICS_WEIGHT);
        assert_eq!(opts.topic.d, GOSSIPSUB_D);
    }

    #[test]
    fn test_max_score_calculation() {
        let opts =
            TopicScoringOptions::new(100_000, 128, &mock_committee(), Duration::from_secs(384));
        let max_score = opts.max_score();

        let expected = (MAX_TIME_IN_MESH_SCORE + MAX_FIRST_DELIVERY_SCORE) * TOTAL_TOPICS_WEIGHT;
        assert_eq!(max_score, expected);
    }

    #[test]
    fn test_sanitize_topic_params() {
        let mut params = TopicScoreParams {
            time_in_mesh_weight: f64::NAN,
            first_message_deliveries_cap: f64::INFINITY,
            mesh_message_deliveries_threshold: f64::NEG_INFINITY,
            ..Default::default()
        };

        TopicScoringOptions::sanitize_topic_params(&mut params);

        assert!(!params.time_in_mesh_weight.is_nan());
        assert!(!params.first_message_deliveries_cap.is_infinite());
        assert!(!params.mesh_message_deliveries_threshold.is_infinite());
    }

    #[test]
    fn test_decay_convergence() {
        let result = decay_convergence(0.9, 10.0).unwrap();
        let expected = 10.0 / 0.1; // 100.0
        assert!((result - expected).abs() < 0.0001);
    }

    #[test]
    fn test_decay_convergence_invalid_factor() {
        assert!(decay_convergence(1.0, 10.0).is_err());
        assert!(decay_convergence(1.5, 10.0).is_err());
    }

    // #[test]
    // fn test_message_rate_calculation() {
    //     let network_config = NetworkConfig {
    //         one_epoch_duration: Duration::from_secs(384),
    //         active_validators: 1000,
    //         total_topics_weight: 10.0,
    //         subnets: 0,
    //     };
    //
    //     let committees = vec![CommitteeId([1; 32]), CommitteeId([2; 32])];
    //
    //     let spec = types::ChainSpec::default();
    //     let rate =
    // TopicScoringConfig::calculate_message_rate_for_subnet::<types::MainnetEthSpec>(
    //         &committees,
    //         &network_config,
    //         &spec,
    //     );
    //
    //     // Should be > 0 for committees with validators
    //     assert!(rate > 0.0);
    // }

    // TopicScoringConfig::calculate_message_rate_for_subnet::<types::MainnetEthSpec>(
    //         &committees,
    //         &network_config,
    //         &spec,
    //     );
    //
    //     // Should be > 0 for committees with validators
    //     assert!(rate > 0.0);
    // }
}
