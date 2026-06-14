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

    /// The CStar fork, the SSV-side rollout of Ethereum's Gloas (ePBS) features.
    ///
    /// Activates directly on top of Alan behavior: Boole is unscheduled and
    /// none of its features (MinHash subnets, AggregatorCommittee roles,
    /// epoch shift) are enabled by CStar.
    ///
    /// Characteristics:
    /// - Subnet topology: inherits Alan behavior (no subnet change at this fork).
    /// - Topic format: `/ssv/<network>/cstar/<subnet>`
    CStar,

    /// The Boole fork introducing MinHash subnet topology.
    ///
    /// Deprioritized indefinitely by SSV core and unscheduled on all networks.
    /// If it ever activates, it must do so after CStar; this variant ordering
    /// makes schedule validation reject a boole epoch at or before cstar's.
    ///
    /// Characteristics:
    /// - Subnet topology: `min(SHA256(operator_id)) % 128`
    /// - Topic format: `/ssv/<network>/boole/<subnet>`
    Boole,
}

impl Fork {
    /// Returns all known forks in chronological order.
    pub const fn all() -> &'static [Fork] {
        &[Fork::Alan, Fork::CStar, Fork::Boole]
    }

    /// Returns the name of this fork as a string.
    pub const fn name(&self) -> &'static str {
        match self {
            Fork::Alan => "alan",
            Fork::CStar => "cstar",
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
            "cstar" => Ok(Fork::CStar),
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
        assert!(Fork::Alan < Fork::CStar);
        assert!(Fork::CStar < Fork::Boole);
    }

    #[test]
    fn test_fork_names() {
        assert_eq!(Fork::Alan.name(), "alan");
        assert_eq!(Fork::Boole.name(), "boole");
        assert_eq!(Fork::CStar.name(), "cstar");
    }

    #[test]
    fn test_fork_display() {
        assert_eq!(format!("{}", Fork::Alan), "alan");
        assert_eq!(format!("{}", Fork::Boole), "boole");
        assert_eq!(format!("{}", Fork::CStar), "cstar");
    }

    #[test]
    fn test_fork_parse() {
        assert_eq!("alan".parse::<Fork>().unwrap(), Fork::Alan);
        assert_eq!("Boole".parse::<Fork>().unwrap(), Fork::Boole);
        assert_eq!("ALAN".parse::<Fork>().unwrap(), Fork::Alan);
        assert_eq!("cstar".parse::<Fork>().unwrap(), Fork::CStar);
        assert_eq!("CSTAR".parse::<Fork>().unwrap(), Fork::CStar);
        assert!("unknown".parse::<Fork>().is_err());
    }

    #[test]
    fn test_all_forks() {
        let all = Fork::all();
        assert_eq!(all.len(), 3);
        assert_eq!(all[0], Fork::Alan);
        assert_eq!(all[1], Fork::CStar);
        assert_eq!(all[2], Fork::Boole);
    }

    #[test]
    fn test_cstar_topic_prefix() {
        let prefix = Fork::CStar.topic_prefix("mainnet");
        assert_eq!(prefix, "/ssv/mainnet/cstar/");

        let prefix_holesky = Fork::CStar.topic_prefix("holesky");
        assert_eq!(prefix_holesky, "/ssv/holesky/cstar/");
    }
}
