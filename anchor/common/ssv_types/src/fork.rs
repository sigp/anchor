//! SSV protocol fork definitions.
//!
//! This module defines the SSV protocol forks and provides utilities for
//! determining which fork is active at a given epoch.

use serde::{Deserialize, Serialize};
use std::fmt;

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
    /// The initial SSV protocol version.
    Genesis,

    /// The Alan fork.
    ///
    /// Characteristics:
    /// - Subnet topology: `committee_id % 128`
    /// - Topic format: `ssv.v2.<subnet>`
    Alan,

    /// The Boole fork introducing MinHash subnet topology.
    ///
    /// Characteristics:
    /// - Subnet topology: `min(SHA256(operator_id)) % 128`
    /// - Topic format: `/ssv/boole/<domaintype>/<subnet>`
    Boole,
}

impl Fork {
    /// Returns all known forks in chronological order.
    pub const fn all() -> &'static [Fork] {
        &[Fork::Genesis, Fork::Alan, Fork::Boole]
    }

    /// Returns the genesis fork (the first fork).
    pub const fn genesis() -> Self {
        Fork::Genesis
    }

    /// Returns the name of this fork as a string.
    pub const fn name(&self) -> &'static str {
        match self {
            Fork::Genesis => "genesis",
            Fork::Alan => "alan",
            Fork::Boole => "boole",
        }
    }

    /// Returns the next fork after this one, if any.
    pub const fn next(&self) -> Option<Fork> {
        match self {
            Fork::Genesis => Some(Fork::Alan),
            Fork::Alan => Some(Fork::Boole),
            Fork::Boole => None,
        }
    }

    /// Returns the previous fork before this one, if any.
    pub const fn previous(&self) -> Option<Fork> {
        match self {
            Fork::Genesis => None,
            Fork::Alan => Some(Fork::Genesis),
            Fork::Boole => Some(Fork::Alan),
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
            "genesis" => Ok(Fork::Genesis),
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
        assert!(Fork::Genesis < Fork::Alan);
        assert!(Fork::Alan < Fork::Boole);
        assert!(Fork::Genesis < Fork::Boole);
    }

    #[test]
    fn test_fork_names() {
        assert_eq!(Fork::Genesis.name(), "genesis");
        assert_eq!(Fork::Alan.name(), "alan");
        assert_eq!(Fork::Boole.name(), "boole");
    }

    #[test]
    fn test_fork_display() {
        assert_eq!(format!("{}", Fork::Genesis), "genesis");
        assert_eq!(format!("{}", Fork::Alan), "alan");
        assert_eq!(format!("{}", Fork::Boole), "boole");
    }

    #[test]
    fn test_fork_parse() {
        assert_eq!("genesis".parse::<Fork>().unwrap(), Fork::Genesis);
        assert_eq!("alan".parse::<Fork>().unwrap(), Fork::Alan);
        assert_eq!("Boole".parse::<Fork>().unwrap(), Fork::Boole);
        assert_eq!("ALAN".parse::<Fork>().unwrap(), Fork::Alan);
        assert!("unknown".parse::<Fork>().is_err());
    }

    #[test]
    fn test_fork_navigation() {
        assert_eq!(Fork::Genesis.next(), Some(Fork::Alan));
        assert_eq!(Fork::Alan.next(), Some(Fork::Boole));
        assert_eq!(Fork::Boole.next(), None);

        assert_eq!(Fork::Genesis.previous(), None);
        assert_eq!(Fork::Alan.previous(), Some(Fork::Genesis));
        assert_eq!(Fork::Boole.previous(), Some(Fork::Alan));
    }

    #[test]
    fn test_all_forks() {
        let all = Fork::all();
        assert_eq!(all.len(), 3);
        assert_eq!(all[0], Fork::Genesis);
        assert_eq!(all[1], Fork::Alan);
        assert_eq!(all[2], Fork::Boole);
    }

    #[test]
    fn test_genesis() {
        assert_eq!(Fork::genesis(), Fork::Genesis);
    }

    #[test]
    fn test_serde() {
        let fork = Fork::Alan;
        let json = serde_json::to_string(&fork).unwrap();
        assert_eq!(json, "\"alan\"");

        let parsed: Fork = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, Fork::Alan);
    }
}
