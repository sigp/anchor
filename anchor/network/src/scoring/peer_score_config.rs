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
        decay_convergence(behaviour_penalty_decay, max_allowed_rate_per_decay_interval)
            .expect("Decay convergence calculation should never fail with valid SSV parameters")
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

/// Calculate score decay factor
///
/// This function implements the decay calculation from the SSV reference implementation.
/// It calculates a decay rate multiplier that, when applied repeatedly,
/// will reduce any initial value to 1% of its original amount over the specified time period.
/// The "1.0" represents a normalized starting point for the mathematical model.
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
/// This function calculates the steady-state value that a score converges to when
/// a constant rate is continuously applied with exponential decay. This is used
/// in peer scoring to determine what score a peer will stabilize at under
/// constant behavior patterns.
///
/// For example, if a peer consistently sends 10 invalid messages per decay interval
/// and the decay factor is 0.9, this function calculates what their penalty score
/// will eventually stabilize at (rather than growing infinitely).
///
/// The mathematical formula is: steady_state = rate / (1 - decay_factor)
///
/// # Arguments
/// * `decay` - The decay factor applied each interval (must be between 0 and 1)
/// * `rate_per_interval` - The rate at which values are added each interval
///
/// # Returns
/// The convergence value (steady-state score), or an error if decay >= 1.0
///
/// # Errors
/// Returns an error if the decay factor is >= 1.0, which would cause mathematical instability
fn decay_convergence(decay: f64, rate_per_interval: f64) -> Result<f64, String> {
    if decay >= 1.0 {
        return Err(format!(
            "Invalid decay rate: {decay}. Decay rate must be < 1.0 to ensure convergence",
        ));
    }

    Ok(rate_per_interval / (1.0 - decay))
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
        let convergence = decay_convergence(decay, rate).unwrap();

        // Should equal rate / (1 - decay) = 10.0 / 0.1 = 100.0
        let expected = rate / (1.0 - decay);
        assert!((convergence - expected).abs() < 0.0001);
    }

    #[test]
    fn test_decay_convergence_invalid_rate_equal_one() {
        let result = decay_convergence(1.0, 10.0);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Invalid decay rate: 1"));
    }

    #[test]
    fn test_decay_convergence_invalid_rate_greater_than_one() {
        let result = decay_convergence(1.5, 10.0);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Invalid decay rate: 1.5"));
    }

    #[test]
    fn test_decay_convergence_boundary_cases() {
        // Test with very small decay (almost no decay)
        let result = decay_convergence(0.01, 5.0).unwrap();
        let expected = 5.0 / 0.99;
        assert!((result - expected).abs() < 0.0001);

        // Test with zero decay (no decay at all)
        let result = decay_convergence(0.0, 5.0).unwrap();
        assert_eq!(result, 5.0);
    }
}
