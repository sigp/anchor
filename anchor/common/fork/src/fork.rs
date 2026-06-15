//! SSV protocol fork definitions.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Topic prefix for Alan fork (legacy format).
pub const ALAN_TOPIC_PREFIX: &str = "ssv.v2.";

/// SSV protocol forks.
///
/// Each fork represents a protocol upgrade that may change various behaviors
/// across the SSV network, such as:
/// - Subnet topology calculation
/// - Topic naming conventions
/// - Message formats
/// - Consensus rules
///
/// Forks are ordered chronologically - earlier variants represent older forks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Fork {
    /// The Alan fork (initial SSV mainnet version).
    ///
    /// Characteristics:
    /// - Subnet topology: `committee_id % 128`
    /// - Topic format: `ssv.v2.<subnet>`
    Alan,

    /// The Boole fork introducing MinHash subnet topology.
    ///
    /// Characteristics:
    /// - Subnet topology: `min(SHA256(operator_id)) % 128`
    /// - Topic format: `/ssv/<network>/boole/<subnet>`
    Boole,
}

impl Fork {
    /// Returns all known forks in chronological order.
    pub const fn all() -> &'static [Fork] {
        &[Fork::Alan, Fork::Boole]
    }

    /// Returns the name of this fork as a string.
    pub const fn name(&self) -> &'static str {
        match self {
            Fork::Alan => "alan",
            Fork::Boole => "boole",
        }
    }

    /// Get the topic prefix for a given fork and subnet.
    ///
    /// - Alan fork: returns the legacy prefix `ssv.v2.`
    /// - Post-Alan forks: returns `/ssv/{network}/{fork}/` format
    pub fn topic_prefix(&self, network_name: &str) -> String {
        match self {
            Fork::Alan => ALAN_TOPIC_PREFIX.to_string(),
            _ => format!("/ssv/{}/{}/", network_name, self.name()),
        }
    }
}

impl fmt::Display for Fork {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl std::str::FromStr for Fork {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "alan" => Ok(Fork::Alan),
            "boole" => Ok(Fork::Boole),
            _ => Err(format!("Unknown fork: {s}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fork_ordering() {
        assert!(Fork::Alan < Fork::Boole);
    }

    #[test]
    fn test_fork_names() {
        assert_eq!(Fork::Alan.name(), "alan");
        assert_eq!(Fork::Boole.name(), "boole");
    }

    #[test]
    fn test_fork_display() {
        assert_eq!(format!("{}", Fork::Alan), "alan");
        assert_eq!(format!("{}", Fork::Boole), "boole");
    }

    #[test]
    fn test_fork_parse() {
        assert_eq!("alan".parse::<Fork>().unwrap(), Fork::Alan);
        assert_eq!("Boole".parse::<Fork>().unwrap(), Fork::Boole);
        assert_eq!("ALAN".parse::<Fork>().unwrap(), Fork::Alan);
        assert!("unknown".parse::<Fork>().is_err());
    }

    #[test]
    fn test_all_forks() {
        let all = Fork::all();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0], Fork::Alan);
        assert_eq!(all[1], Fork::Boole);
    }

    #[test]
    fn test_boole_topic_prefix() {
        let prefix = Fork::Boole.topic_prefix("mainnet");
        assert_eq!(prefix, "/ssv/mainnet/boole/");

        let prefix_holesky = Fork::Boole.topic_prefix("holesky");
        assert_eq!(prefix_holesky, "/ssv/holesky/boole/");
    }
}
