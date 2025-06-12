//! Peer scoring configuration matching the SSV reference implementation.
//!
//! This module provides peer scoring parameters and thresholds that align with
//! the Go implementation to ensure consistent network behavior across implementations.

use std::time::Duration;

// Peer scoring thresholds (matching SSV reference implementation)
pub const GOSSIP_THRESHOLD: f64 = -4000.0;
pub const PUBLISH_THRESHOLD: f64 = -8000.0;
pub const GRAYLIST_THRESHOLD: f64 = -16000.0;
pub const ACCEPT_PX_THRESHOLD: f64 = 100.0;
pub const OPPORTUNISTIC_GRAFT_THRESHOLD: f64 = 5.0;

// Overall peer scoring parameters
pub const TOPIC_SCORE_CAP: f64 = 32.72;
pub const DECAY_TO_ZERO: f64 = 0.01;

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
        decay_convergence(behaviour_penalty_decay, max_allowed_rate_per_decay_interval)
            - BEHAVIOUR_PENALTY_THRESHOLD;
    let behaviour_penalty_weight = GOSSIP_THRESHOLD / (target_val * target_val);

    let retain_score = Duration::from_secs(100 * 32 * 12); // 100 epochs

    gossipsub::PeerScoreParams {
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
        ..Default::default()
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

/// Calculate score decay factor
///
/// This function implements the decay calculation from the SSV reference implementation
/// to ensure consistent behavior across different client implementations.
///
/// # Arguments
/// * `lifetime` - How long the score should take to decay
/// * `decay_interval` - How often decay is applied
///
/// # Returns
/// The decay factor to be applied at each interval
fn calculate_score_decay_factor(lifetime: Duration, decay_interval: Duration) -> f64 {
    let ticks = lifetime.as_secs_f64() / decay_interval.as_secs_f64();
    DECAY_TO_ZERO.powf(1.0 / ticks)
}

/// Calculate decay convergence
///
/// This calculates the steady-state value when a constant rate is applied
/// with exponential decay.
///
/// # Arguments
/// * `decay` - The decay factor applied each interval
/// * `rate_per_interval` - The rate at which values are added each interval
///
/// # Returns
/// The convergence value (steady-state)
fn decay_convergence(decay: f64, rate_per_interval: f64) -> f64 {
    rate_per_interval / (1.0 - decay)
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

    #[test]
    fn test_score_decay() {
        let lifetime = Duration::from_secs(100);
        let interval = Duration::from_secs(10);
        let decay = calculate_score_decay_factor(lifetime, interval);

        // Should be between 0 and 1
        assert!(decay > 0.0 && decay < 1.0);

        // With 10 intervals, should decay to DECAY_TO_ZERO
        let final_value = decay.powi(10);
        assert!((final_value - DECAY_TO_ZERO).abs() < 0.001);
    }

    #[test]
    fn test_decay_convergence() {
        let decay = 0.9;
        let rate = 10.0;
        let convergence = decay_convergence(decay, rate);

        // Should equal rate / (1 - decay)
        assert_eq!(convergence, 100.0);
    }
}
