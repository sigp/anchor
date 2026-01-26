//! Gossipsub topic utilities for SSV network.
//!
//! This module provides utilities for creating and parsing gossipsub topics
//! for different SSV forks. Topic formats:
//! - Alan (legacy): `ssv.v2.<subnet_id>`
//! - Boole and later: `/ssv/<network>/<fork>/<subnet_id>`

use fork::Fork;
use gossipsub::{IdentTopic, TopicHash};

use crate::{SUBNET_COUNT, SubnetId};

/// Create a gossipsub topic for a subnet using the given prefix.
///
/// The prefix should include the trailing separator (e.g., "ssv.v2." or "/ssv/mainnet/boole/").
pub fn create_topic(prefix: &str, subnet: SubnetId) -> IdentTopic {
    IdentTopic::new(format!("{}{}", prefix, *subnet))
}

/// Result of parsing a topic hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedTopic {
    pub subnet_id: SubnetId,
    pub fork: Fork,
    /// Network name extracted from the topic.
    /// Only present for post-Alan topics (e.g., `/ssv/mainnet/boole/42`).
    /// Alan topics use legacy format without network name.
    pub network: Option<String>,
}

/// Parse a topic hash to extract the subnet ID, fork, and network name.
///
/// Supports multiple topic formats:
/// - Alan (legacy): `ssv.v2.<subnet_id>` - no network name
/// - Post-Alan forks (Boole, etc.): `/ssv/<network>/<fork>/<subnet_id>` - includes network name
///
/// Returns `None` if the topic doesn't match a known format or the subnet ID is out of range.
#[must_use]
pub fn parse_topic(topic: &TopicHash) -> Option<ParsedTopic> {
    parse_topic_str(topic.as_str())
}

/// Internal function to parse a topic string.
fn parse_topic_str(s: &str) -> Option<ParsedTopic> {
    // Try Alan format: ssv.v2.<subnet_id>
    if let Some(suffix) = s.strip_prefix("ssv.v2.") {
        let subnet_num: u64 = suffix.parse().ok()?;
        let subnet_id = parse_and_validate_subnet(subnet_num)?;
        return Some(ParsedTopic {
            subnet_id,
            fork: Fork::Alan,
            network: None,
        });
    }

    // Try post-Alan format: /ssv/<network>/<fork>/<subnet_id>
    // This format is used by Boole and all future forks.
    let parts: Vec<&str> = s.split('/').collect();
    if let ["", "ssv", network, fork_name, subnet_str] = parts.as_slice() {
        // Parse fork name dynamically to support future forks
        let fork: Fork = fork_name.parse().ok()?;
        // Alan uses legacy format, not this path
        if fork == Fork::Alan {
            return None;
        }
        let subnet_num: u64 = subnet_str.parse().ok()?;
        let subnet_id = parse_and_validate_subnet(subnet_num)?;
        return Some(ParsedTopic {
            subnet_id,
            fork,
            network: Some((*network).to_string()),
        });
    }

    None
}

/// Parse and validate a subnet number, ensuring it's within the valid range.
fn parse_and_validate_subnet(subnet_num: u64) -> Option<SubnetId> {
    (subnet_num < SUBNET_COUNT as u64).then(|| SubnetId::from(subnet_num))
}

/// Extract just the subnet ID from a topic, ignoring which fork it belongs to.
///
/// This is a convenience function for cases where you only care about
/// the subnet ID, not the fork (e.g., tracking peer subscriptions).
///
/// # Warning
///
/// This function discards fork information. For message validation or routing
/// decisions that depend on the fork, use [`parse_topic`] instead to get both
/// the subnet ID and the fork.
#[must_use = "Fork information is discarded. Use parse_topic() if fork context matters for validation or routing"]
pub fn parse_subnet_id(topic: &TopicHash) -> Option<SubnetId> {
    parse_topic(topic).map(|p| p.subnet_id)
}

/// Extract subnet ID from a topic string.
///
/// This is a convenience function for extracting subnet ID from a raw topic string
/// (e.g., when the topic comes from a channel as a String rather than a TopicHash).
///
/// Supports both Alan format (`ssv.v2.<subnet_id>`) and post-Alan format
/// (`/ssv/<network>/<fork>/<subnet_id>`).
#[must_use]
pub fn extract_subnet_id(topic_str: &str) -> Option<u64> {
    parse_topic_str(topic_str).map(|p| *p.subnet_id)
}

#[cfg(test)]
mod tests {
    use ssv_network_config::ALAN_TOPIC_PREFIX;

    use super::*;

    // Test network names
    const MAINNET: &str = "mainnet";
    const HOLESKY: &str = "holesky";

    // Test subnet IDs
    const TEST_SUBNET_ID: u64 = 42;
    const ALTERNATE_SUBNET_ID: u64 = 100;
    const OUT_OF_RANGE_SUBNET_ID: u64 = 200;

    /// Constructs the expected Boole topic string for a network and subnet.
    fn expected_boole_topic(network: &str, subnet_id: u64) -> String {
        format!("/ssv/{}/boole/{}", network, subnet_id)
    }

    /// Constructs the expected Alan topic string for a subnet.
    fn expected_alan_topic(subnet_id: u64) -> String {
        format!("ssv.v2.{}", subnet_id)
    }

    // ==================== create_topic tests ====================

    #[test]
    fn test_create_topic_alan_format_uses_legacy_prefix() {
        // Arrange
        let subnet = SubnetId::from(TEST_SUBNET_ID);

        // Act
        let topic = create_topic(ALAN_TOPIC_PREFIX, subnet);

        // Assert
        assert_eq!(
            topic.hash().as_str(),
            expected_alan_topic(TEST_SUBNET_ID),
            "Alan topic should use legacy ssv.v2 prefix"
        );
    }

    #[test]
    fn test_create_topic_boole_format_uses_network_prefix() {
        // Arrange
        let subnet = SubnetId::from(TEST_SUBNET_ID);
        let boole_prefix = format!("/ssv/{}/boole/", MAINNET);

        // Act
        let topic = create_topic(&boole_prefix, subnet);

        // Assert
        assert_eq!(
            topic.hash().as_str(),
            expected_boole_topic(MAINNET, TEST_SUBNET_ID),
            "Boole topic should use network-specific prefix"
        );
    }

    // ==================== parse_topic tests ====================

    #[test]
    fn test_parse_topic_alan_format_extracts_subnet_and_fork() {
        // Arrange
        let topic = IdentTopic::new(expected_alan_topic(TEST_SUBNET_ID)).hash();

        // Act
        let parsed = parse_topic(&topic);

        // Assert
        let parsed = parsed.expect("should parse valid Alan topic");
        assert_eq!(
            *parsed.subnet_id, TEST_SUBNET_ID,
            "should extract correct subnet ID"
        );
        assert_eq!(parsed.fork, Fork::Alan, "should identify Alan fork");
        assert_eq!(parsed.network, None, "Alan format has no network name");
    }

    #[test]
    fn test_parse_topic_boole_format_extracts_subnet_fork_and_network() {
        // Arrange
        let topic = IdentTopic::new(expected_boole_topic(MAINNET, TEST_SUBNET_ID)).hash();

        // Act
        let parsed = parse_topic(&topic);

        // Assert
        let parsed = parsed.expect("should parse valid Boole topic");
        assert_eq!(
            *parsed.subnet_id, TEST_SUBNET_ID,
            "should extract correct subnet ID"
        );
        assert_eq!(parsed.fork, Fork::Boole, "should identify Boole fork");
        assert_eq!(
            parsed.network,
            Some(MAINNET.to_string()),
            "should extract network name"
        );
    }

    #[test]
    fn test_parse_topic_boole_format_works_with_different_networks() {
        // Arrange
        let topic = IdentTopic::new(expected_boole_topic(HOLESKY, ALTERNATE_SUBNET_ID)).hash();

        // Act
        let parsed = parse_topic(&topic);

        // Assert
        let parsed = parsed.expect("should parse Boole topic for any network");
        assert_eq!(
            *parsed.subnet_id, ALTERNATE_SUBNET_ID,
            "should extract correct subnet ID"
        );
        assert_eq!(parsed.fork, Fork::Boole, "should identify Boole fork");
        assert_eq!(
            parsed.network,
            Some(HOLESKY.to_string()),
            "should extract network name for different networks"
        );
    }

    // ==================== parse_topic invalid input tests ====================

    #[test]
    fn test_parse_topic_returns_none_for_invalid_format() {
        // Arrange
        let topic = IdentTopic::new("invalid").hash();

        // Act
        let result = parse_topic(&topic);

        // Assert
        assert!(
            result.is_none(),
            "should reject topic with unrecognized format"
        );
    }

    #[test]
    fn test_parse_topic_returns_none_for_unknown_fork() {
        // Arrange
        let topic = IdentTopic::new(format!("/ssv/{}/unknown/{}", MAINNET, TEST_SUBNET_ID)).hash();

        // Act
        let result = parse_topic(&topic);

        // Assert
        assert!(
            result.is_none(),
            "should reject topic with unknown fork name"
        );
    }

    #[test]
    fn test_parse_topic_returns_none_for_missing_subnet() {
        // Arrange
        let topic = IdentTopic::new(format!("/ssv/{}/boole", MAINNET)).hash();

        // Act
        let result = parse_topic(&topic);

        // Assert
        assert!(
            result.is_none(),
            "should reject topic with missing subnet ID"
        );
    }

    #[test]
    fn test_parse_topic_returns_none_for_non_numeric_subnet() {
        // Arrange
        let topic = IdentTopic::new("ssv.v2.abc").hash();

        // Act
        let result = parse_topic(&topic);

        // Assert
        assert!(
            result.is_none(),
            "should reject topic with non-numeric subnet ID"
        );
    }

    #[test]
    fn test_parse_topic_returns_none_for_out_of_range_subnet() {
        // Arrange
        let topic = IdentTopic::new(expected_alan_topic(OUT_OF_RANGE_SUBNET_ID)).hash();

        // Act
        let result = parse_topic(&topic);

        // Assert
        assert!(
            result.is_none(),
            "should reject topic with subnet ID exceeding SUBNET_COUNT"
        );
    }

    // ==================== parse_subnet_id tests ====================

    #[test]
    fn test_parse_subnet_id_extracts_id_from_alan_topic() {
        // Arrange
        let topic = IdentTopic::new(expected_alan_topic(TEST_SUBNET_ID)).hash();

        // Act
        let result = parse_subnet_id(&topic);

        // Assert
        assert_eq!(
            *result.expect("should extract subnet ID"),
            TEST_SUBNET_ID,
            "should return correct subnet ID from Alan topic"
        );
    }

    #[test]
    fn test_parse_subnet_id_extracts_id_from_boole_topic() {
        // Arrange
        let topic = IdentTopic::new(expected_boole_topic(MAINNET, TEST_SUBNET_ID)).hash();

        // Act
        let result = parse_subnet_id(&topic);

        // Assert
        assert_eq!(
            *result.expect("should extract subnet ID"),
            TEST_SUBNET_ID,
            "should return correct subnet ID from Boole topic"
        );
    }
}
