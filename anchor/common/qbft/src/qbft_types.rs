//! A collection of types used by the QBFT modules
use std::{cmp::Eq, fmt::Debug, hash::Hash};

use derive_more::{Deref, From};
use indexmap::IndexSet;
use ssv_types::{
    OperatorId, Round,
    consensus::{QbftMessage, UnsignedSSVMessage},
    message::SignedSSVMessage,
};
use types::Hash256;

/// Generic LeaderFunction trait to allow for future implementations of the QBFT module
pub trait LeaderFunction {
    /// Returns true if we are the leader
    fn leader_function(
        &self,
        operator_id: &OperatorId,
        round: Round,
        instance_height: InstanceHeight,
        committee: &IndexSet<OperatorId>,
    ) -> bool;
}

#[derive(Debug, Clone, Default)]
pub struct DefaultLeaderFunction {}

impl LeaderFunction for DefaultLeaderFunction {
    fn leader_function(
        &self,
        operator_id: &OperatorId,
        round: Round,
        instance_height: InstanceHeight,
        committee: &IndexSet<OperatorId>,
    ) -> bool {
        *operator_id
            == *committee
                .get_index(
                    ((round.get() - Round::default().get()) + *instance_height) % committee.len(),
                )
                .expect("slice bounds kept by modulo length")
    }
}

// Wrapped qbft message is a wrapper around both a signed ssv message, and the underlying qbft
// message.
#[derive(Debug, Clone)]
pub struct WrappedQbftMessage {
    pub signed_message: SignedSSVMessage,
    pub qbft_message: QbftMessage,
}

// Wrapped qbft message is a wrapper around both an unsigned ssv message, and the underlying qbft
// message.
#[derive(Debug, Clone)]
pub struct UnsignedWrappedQbftMessage {
    pub unsigned_message: UnsignedSSVMessage,
    pub qbft_message: QbftMessage,
}

/// The instance height behaves like an "ID" for the QBFT instance. It is used to uniquely identify
/// different instances, that have the same operator id.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash, From, Deref)]
pub struct InstanceHeight(usize);

#[derive(Debug, Clone, Copy)]
pub enum InstanceState {
    /// Awaiting a propose from a leader
    AwaitingProposal,
    /// Awaiting consensus on PREPARE messages
    Prepare { proposal_root: Hash256 },
    /// Awaiting consensus on COMMIT messages
    Commit { proposal_root: Hash256 },
    /// We have sent a round change message
    SentRoundChange,
    /// The consensus instance is complete
    Complete,
    /// We have reached consensus on a round change
    RoundChangeConsensus,
}

impl From<InstanceState> for u8 {
    fn from(state: InstanceState) -> u8 {
        match state {
            InstanceState::AwaitingProposal => 0,
            InstanceState::Prepare { .. } => 1,
            InstanceState::Commit { .. } => 2,
            InstanceState::SentRoundChange => 4,
            InstanceState::Complete => 5,
            InstanceState::RoundChangeConsensus => 6,
        }
    }
}

/// Type definitions for the allowable messages
/// This holds the consensus data for a given round.
#[derive(Debug, Clone)]
pub struct ConsensusData<D> {
    /// The round that this data corresponds to
    pub round: Round,
    /// The actual value we reached consensus on.
    pub data: D,
}

#[derive(Debug, Clone)]
/// The consensus instance has finished.
pub enum Completed<D> {
    /// The instance has timed out.
    TimedOut,
    /// Consensus was reached on the provided data.
    Success(D),
}
