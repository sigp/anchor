//! SSV protocol fork management.
//!
//! This crate provides types and utilities for managing SSV protocol forks:
//!
//! - [`Fork`]: Enum representing SSV protocol versions (Alan, Boole)
//! - [`ForkSchedule`]: Manages fork activation epochs and transition timing
//! - [`ForkConfig`]: Complete configuration for a fork including topic prefix
//! - [`ForkPhase`]: Fork transition events for components
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

pub use fork::{ALAN_TOPIC_PREFIX, Fork};
pub use monitor::{ForkPhase, ForkPhaseSender};
pub use schedule::{FORK_PREPARATION_EPOCHS, ForkConfig, ForkSchedule, SUBSEQUENT_WINDOW_SLOTS};
