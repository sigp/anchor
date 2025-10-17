//! Slashing protection trait and implementations.
//!
//! This module provides a trait abstraction over slashing protection, allowing for:
//! - Production use with Lighthouse's SlashingDatabase (supports both file-based and in-memory
//!   SQLite)
//! - Testing with a no-op implementation that doesn't require any database operations
//!
//! The trait enables dependency injection and makes code testable without needing
//! slashing protection infrastructure.

use slashing_protection::SlashingDatabase;
use types::PublicKeyBytes;

/// Trait for slashing protection implementations.
///
/// Provides an abstraction over slashing protection operations, allowing for
/// different implementations (e.g., file-based, in-memory, no-op for tests).
///
/// Currently only includes methods used by the eth crate. Additional methods
/// (check_and_insert_block_proposal, check_and_insert_attestation, pruning, etc.)
/// can be added when needed for other crates.
pub trait SlashingProtection: Send + Sync {
    /// Register a new validator in the slashing protection database.
    ///
    /// This must be called before the validator can sign any blocks or attestations.
    /// Returns an error if registration fails.
    fn register_validator(&self, public_key: PublicKeyBytes) -> Result<(), String>;
}

/// Wrapper implementation for Lighthouse's SlashingDatabase.
///
/// This implementation uses SQLite for slashing protection data, which can be either
/// file-based (persistent) or in-memory depending on how the SlashingDatabase was created.
impl SlashingProtection for SlashingDatabase {
    fn register_validator(&self, public_key: PublicKeyBytes) -> Result<(), String> {
        SlashingDatabase::register_validator(self, public_key)
            .map_err(|e| format!("Failed to register validator: {e:?}"))
    }
}

/// No-op slashing protection implementation for testing.
///
/// This implementation always allows all operations and performs no persistence.
/// It should **only** be used in test code where actual slashing protection is not required.
#[cfg(feature = "test-utils")]
#[derive(Debug, Clone, Copy, Default)]
pub struct NoOpSlashingProtection;

#[cfg(feature = "test-utils")]
impl NoOpSlashingProtection {
    /// Create a new no-op slashing protection instance.
    pub fn new() -> Self {
        Self
    }
}

#[cfg(feature = "test-utils")]
impl SlashingProtection for NoOpSlashingProtection {
    fn register_validator(&self, _public_key: PublicKeyBytes) -> Result<(), String> {
        Ok(())
    }
}
