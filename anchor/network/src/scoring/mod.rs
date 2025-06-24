use std::time::Duration;

use crate::scoring::peer_score_config::DECAY_TO_ZERO;

pub mod message_rate;
pub mod peer_score_config;
pub mod topic_score_config;

/// Calculate the convergence value for exponential decay
fn decay_convergence(decay_factor: f64, rate_per_interval: f64) -> Result<f64, String> {
    if decay_factor >= 1.0 {
        return Err(format!(
            "Invalid decay factor: {}. Must be < 1.0 for convergence",
            decay_factor
        ));
    }
    Ok(rate_per_interval / (1.0 - decay_factor))
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

/// Calculate the threshold where decay reaches a target value
fn decay_threshold(decay_factor: f64, target_value: f64) -> Result<f64, String> {
    if decay_factor >= 1.0 {
        return Err(format!(
            "Invalid decay factor: {}. Must be < 1.0",
            decay_factor
        ));
    }
    if target_value <= 0.0 {
        return Err("Target value must be positive".to_string());
    }
    Ok(target_value / (1.0 - decay_factor))
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn test_decay_threshold() {
        let result = decay_threshold(0.8, 5.0).unwrap();
        let expected = 5.0 / 0.2; // 25.0
        assert!((result - expected).abs() < 0.0001);
    }
}
