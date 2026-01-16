//! Fork context with derived values.
//!
//! This module provides `ForkContext` which holds the current fork along with
//! pre-computed derived values like topic prefix.

use crate::Fork;

/// Topic prefix for Alan fork (legacy format).
pub const ALAN_TOPIC_PREFIX: &str = "ssv.v2.";

/// Current fork context with derived values.
///
/// This struct holds the current active fork along with the pre-computed
/// topic prefix. It's designed to be shared via `tokio::sync::watch` for
/// efficient read access across multiple components.
///
/// Components receive a `watch::Receiver<ForkContext>` and can:
/// - Read current values: `fork_ctx.borrow().topic_prefix()`
/// - Wait for changes: `fork_ctx.changed().await`
///
/// Fields are private to maintain the invariant that `topic_prefix` always
/// matches the `fork` value.
#[derive(Clone, Debug)]
pub struct ForkContext {
    /// The currently active fork.
    fork: Fork,
    /// Topic prefix for the current fork (e.g., "ssv.v2." or "/ssv/mainnet/boole/").
    topic_prefix: String,
}

impl ForkContext {
    /// Create a new fork context with pre-computed topic prefix.
    pub fn new(fork: Fork, network_name: &str) -> Self {
        let topic_prefix = topic_prefix_for_fork(fork, network_name);
        Self { fork, topic_prefix }
    }

    /// Get the currently active fork.
    pub fn fork(&self) -> Fork {
        self.fork
    }

    /// Get the topic prefix for the current fork.
    pub fn topic_prefix(&self) -> &str {
        &self.topic_prefix
    }
}

/// Get the topic prefix for a given fork.
///
/// - Alan fork: returns the legacy prefix `ssv.v2.`
/// - Post-Alan forks: returns `/ssv/{network}/{fork}/` format
pub fn topic_prefix_for_fork(fork: Fork, network_name: &str) -> String {
    match fork {
        Fork::Alan => ALAN_TOPIC_PREFIX.to_string(),
        _ => format!("/ssv/{}/{}/", network_name, fork.name()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Test network names
    const MAINNET: &str = "mainnet";
    const HOLESKY: &str = "holesky";

    /// Constructs the expected topic prefix for post-Alan forks.
    fn expected_boole_prefix(network: &str) -> String {
        format!("/ssv/{}/boole/", network)
    }

    // ==================== topic_prefix_for_fork tests ====================

    #[test]
    fn test_topic_prefix_for_fork_alan_returns_legacy_prefix_regardless_of_network() {
        // Arrange
        let networks = [MAINNET, HOLESKY];

        for network in networks {
            // Act
            let result = topic_prefix_for_fork(Fork::Alan, network);

            // Assert
            assert_eq!(
                result, ALAN_TOPIC_PREFIX,
                "Alan fork should always use legacy prefix for any network"
            );
        }
    }

    #[test]
    fn test_topic_prefix_for_fork_boole_returns_network_specific_prefix() {
        // Arrange
        let test_cases = [
            (MAINNET, expected_boole_prefix(MAINNET)),
            (HOLESKY, expected_boole_prefix(HOLESKY)),
        ];

        for (network, expected_prefix) in test_cases {
            // Act
            let result = topic_prefix_for_fork(Fork::Boole, network);

            // Assert
            assert_eq!(
                result, expected_prefix,
                "Boole fork should use network-specific prefix format"
            );
        }
    }

    // ==================== ForkContext::new tests ====================

    #[test]
    fn test_fork_context_new_alan_sets_fork_and_legacy_prefix() {
        // Arrange
        let fork = Fork::Alan;
        let network = MAINNET;

        // Act
        let ctx = ForkContext::new(fork, network);

        // Assert
        assert_eq!(ctx.fork(), Fork::Alan);
        assert_eq!(ctx.topic_prefix(), ALAN_TOPIC_PREFIX);
    }

    #[test]
    fn test_fork_context_new_boole_sets_fork_and_network_prefix() {
        // Arrange
        let fork = Fork::Boole;
        let network = MAINNET;

        // Act
        let ctx = ForkContext::new(fork, network);

        // Assert
        assert_eq!(ctx.fork(), Fork::Boole);
        assert_eq!(ctx.topic_prefix(), expected_boole_prefix(network));
    }
}
