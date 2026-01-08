//! SSV protocol fork management.
//!
//! This crate provides types and utilities for managing SSV protocol forks:
//!
//! - [`Fork`]: Enum representing SSV protocol versions (Genesis, Alan, Boole)
//! - [`ForkSchedule`]: Manages fork activation epochs and transition timing
//!
//! # Fork Transitions
//!
//! A fork is a protocol-wide upgrade that may change multiple subsystems:
//! - Subnet topology calculation
//! - Topic naming conventions
//! - Message formats
//! - Consensus rules
//!
//! The fork infrastructure separates two concerns:
//! - **"When"**: The schedule manages activation epochs and preparation windows
//! - **"What"**: Each subsystem queries the active fork to determine behavior

mod fork;
pub mod monitor;
mod schedule;

pub use fork::Fork;
pub use schedule::{FORK_PREPARATION_EPOCHS, ForkSchedule};
