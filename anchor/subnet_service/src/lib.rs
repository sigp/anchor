//! Subnet service for SSV network topology.
//!
//! This crate provides:
//! - Subnet identification and calculation algorithms (`SubnetId`)
//! - Background service for managing subnet subscriptions
//! - Message rate calculation for gossipsub topic scoring

pub mod message_rate;
mod scoring;
mod service;
mod subnet;

pub use scoring::{calculate_message_rate_for_subnet, get_committee_info_for_subnet};
pub use service::start_subnet_service;
pub use subnet::{SUBNET_COUNT, SubnetBits, SubnetCalculationError, SubnetEvent, SubnetId};
