mod message_rate;
pub(crate) mod peer_score_config;
pub(crate) mod topic_score_config;

/// Calculate the threshold where decay reaches a target value
pub(crate) fn decay_threshold(decay_factor: f64, target_value: f64) -> Result<f64, String> {
    if decay_factor >= 1.0 {
        return Err(format!(
            "Invalid decay factor: {decay_factor}. Must be < 1.0"
        ));
    }
    if target_value <= 0.0 {
        return Err("Target value must be positive".to_string());
    }
    Ok(target_value / (1.0 - decay_factor))
}
