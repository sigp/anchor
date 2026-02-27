//! Subnet service for SSV network topology.
//!
//! This crate provides:
//! - Subnet identification and calculation algorithms (`SubnetId`)
//! - Background service for managing subnet subscriptions
//! - Message rate calculation for gossipsub topic scoring
//! - Fork-aware routing for subnet topology transitions (`TopicRouter`)
//! - Topic utilities for creating and parsing gossipsub topics

pub mod message_rate;
mod routing;
mod scoring;
mod service;
mod subnet;
mod subscriptions;
pub mod topic;

pub use routing::TopicRouter;
pub use service::{SubnetService, SubnetServiceError, start_subnet_service};
pub use subnet::{
    SUBNET_COUNT, SUBNET_COUNT_NZ, SubnetBits, SubnetCalculationError, SubnetId, TopicEvent,
};
