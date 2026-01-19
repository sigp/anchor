//! Subnet service for SSV network topology.
//!
//! This crate provides:
//! - Subnet identification and calculation algorithms (`SubnetId`)
//! - Background service for managing subnet subscriptions
//! - Message rate calculation for gossipsub topic scoring
//! - Fork-aware routing for subnet topology transitions
//! - Topic utilities for creating and parsing gossipsub topics

pub mod message_rate;
mod scoring;
mod service;
mod subnet;
pub mod topic;

pub use scoring::{calculate_message_rate_for_subnet, get_committee_info_for_subnet};
pub use service::{SubnetService, SubnetServiceError, start_subnet_service};
pub use subnet::{
    SUBNET_COUNT, SUBNET_COUNT_NZ, SubnetBits, SubnetCalculationError, SubnetEvent, SubnetId,
    subnet_for_committee,
};
