//! Peer scoring configuration matching the SSV reference implementation.
//!
//! This module provides peer scoring parameters and thresholds that align with
//! the Go implementation to ensure consistent network behavior across implementations.

use std::time::Duration;

use crate::scoring::{calculate_score_decay_factor, decay_convergence};

// Peer scoring thresholds (matching SSV reference implementation)
pub const GOSSIP_THRESHOLD: f64 = -4000.0;
pub const PUBLISH_THRESHOLD: f64 = -8000.0;
pub const GRAYLIST_THRESHOLD: f64 = -16000.0;
pub const ACCEPT_PX_THRESHOLD: f64 = 100.0;
pub const OPPORTUNISTIC_GRAFT_THRESHOLD: f64 = 5.0;

// Overall peer scoring parameters
pub const TOPIC_SCORE_CAP: f64 = 32.72;
pub const DECAY_TO_ZERO: f64 = 0.01;
pub const RETAIN_SCORE_EPOCH_MULTIPLIER: u32 = 100;

// P5
pub const APP_SPECIFIC_WEIGHT: f64 = 0.0;

// P6 - IP Colocation parameters
pub const IP_COLOCATION_FACTOR_THRESHOLD: f64 = 10.0;
pub const IP_COLOCATION_FACTOR_WEIGHT: f64 = -TOPIC_SCORE_CAP;

// P7 - Behavior penalty parameters
pub const BEHAVIOUR_PENALTY_THRESHOLD: f64 = 6.0;

/// Calculate peer score parameters matching SSV reference implementation
///
/// # Arguments
/// * `one_epoch` - Duration of one epoch (32 slots * 12 seconds by default)
///
/// # Returns
/// Configured `PeerScoreParams` for gossipsub
pub fn peer_score_params(one_epoch: Duration) -> gossipsub::PeerScoreParams {
    let decay_interval = one_epoch; // Use one epoch as decay interval

    // P7 calculation - behavior penalty decay
    let behaviour_penalty_decay = calculate_score_decay_factor(one_epoch * 10, decay_interval);
    let max_allowed_rate_per_decay_interval = 10.0;
    let target_val =
        decay_convergence(behaviour_penalty_decay, max_allowed_rate_per_decay_interval).unwrap()
            - BEHAVIOUR_PENALTY_THRESHOLD;
    let behaviour_penalty_weight = GOSSIP_THRESHOLD / (target_val * target_val);

    let retain_score = RETAIN_SCORE_EPOCH_MULTIPLIER * one_epoch; // 100 epochs

    gossipsub::PeerScoreParams {
        topics: Default::default(), // TODO https://github.com/sigp/anchor/issues/371
        topic_score_cap: TOPIC_SCORE_CAP,
        decay_interval,
        decay_to_zero: DECAY_TO_ZERO,
        retain_score,
        app_specific_weight: APP_SPECIFIC_WEIGHT,
        ip_colocation_factor_weight: IP_COLOCATION_FACTOR_WEIGHT,
        ip_colocation_factor_threshold: IP_COLOCATION_FACTOR_THRESHOLD,
        behaviour_penalty_weight,
        behaviour_penalty_threshold: BEHAVIOUR_PENALTY_THRESHOLD,
        behaviour_penalty_decay,
        ..Default::default() /* Use default values for slow_peer_decay, slow_peer_weight,
                              * slow_peer_threshold and ip_colocation_factor_whitelist for now */
    }
}

/// Calculate peer score thresholds matching SSV reference implementation
///
/// # Returns
/// Configured `PeerScoreThresholds` for gossipsub
pub fn peer_score_thresholds() -> gossipsub::PeerScoreThresholds {
    gossipsub::PeerScoreThresholds {
        gossip_threshold: GOSSIP_THRESHOLD,
        publish_threshold: PUBLISH_THRESHOLD,
        graylist_threshold: GRAYLIST_THRESHOLD,
        accept_px_threshold: ACCEPT_PX_THRESHOLD,
        opportunistic_graft_threshold: OPPORTUNISTIC_GRAFT_THRESHOLD,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_peer_score_thresholds() {
        let thresholds = peer_score_thresholds();

        assert_eq!(thresholds.gossip_threshold, GOSSIP_THRESHOLD);
        assert_eq!(thresholds.publish_threshold, PUBLISH_THRESHOLD);
        assert_eq!(thresholds.graylist_threshold, GRAYLIST_THRESHOLD);
        assert_eq!(thresholds.accept_px_threshold, ACCEPT_PX_THRESHOLD);
        assert_eq!(
            thresholds.opportunistic_graft_threshold,
            OPPORTUNISTIC_GRAFT_THRESHOLD
        );
    }
}
