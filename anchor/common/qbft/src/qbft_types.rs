//! A collection of types used by the QBFT modules
use std::{
    cmp::Eq,
    fmt::{Debug, Display, Formatter},
    hash::Hash,
    num::NonZeroU64,
};

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

/// Default leader function implementation with configurable epoch-based rotation.
#[derive(Debug, Clone)]
pub struct DefaultLeaderFunction {
    slots_per_epoch: NonZeroU64,
    include_epoch_shift: bool,
}

impl DefaultLeaderFunction {
    pub fn new(slots_per_epoch: NonZeroU64, include_epoch_shift: bool) -> Self {
        Self {
            slots_per_epoch,
            include_epoch_shift,
        }
    }
}

impl Default for DefaultLeaderFunction {
    fn default() -> Self {
        Self {
            slots_per_epoch: NonZeroU64::new(32).expect("slots_per_epoch is non-zero"),
            include_epoch_shift: false,
        }
    }
}

impl LeaderFunction for DefaultLeaderFunction {
    fn leader_function(
        &self,
        operator_id: &OperatorId,
        round: Round,
        instance_height: InstanceHeight,
        committee: &IndexSet<OperatorId>,
    ) -> bool {
        // Sort the committee to ensure deterministic leader selection
        let mut sorted_committee = committee.clone();
        sorted_committee.sort_unstable();

        let height = *instance_height;

        // Calculate epoch shift if enabled (Boole fork)
        let eth_epoch = if self.include_epoch_shift {
            height / self.slots_per_epoch.get() as usize
        } else {
            0
        };

        let index =
            (round.get() - Round::default().get() + height + eth_epoch) % sorted_committee.len();

        *operator_id
            == *sorted_committee
                .get_index(index)
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

// This impl is meant for displaying messages in debug logs, where we usually do not need to know,
// e.g., the exact byte values of signatures. The `Debug` impl remains fully featured for tracing
// logs or other special cases.
impl Display for WrappedQbftMessage {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let mut f = f.debug_struct("WrappedQbftMessage");
        f.field("operator_ids", &self.signed_message.operator_ids())
            .field("full_data", &!self.signed_message.full_data().is_empty());
        self.qbft_message.format_fields(&mut f);
        f.finish()
    }
}

impl<'a> From<&'a WrappedQbftMessage> for &'a SignedSSVMessage {
    fn from(value: &WrappedQbftMessage) -> &SignedSSVMessage {
        &value.signed_message
    }
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

/// A payload-free view of [`InstanceState`].
///
/// `InstanceState` carries a `proposal_root` payload on its `Prepare` and `Commit` variants.
/// This kind identifies *which* state an instance is in without that instance-specific detail,
/// supporting state-machine decision branching where only the variant matters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InstanceStateKind {
    /// Awaiting a proposal from a leader.
    AwaitingProposal,
    /// Awaiting consensus on PREPARE messages.
    Prepare,
    /// Awaiting consensus on COMMIT messages.
    Commit,
    /// We have sent a round change message.
    SentRoundChange,
    /// The consensus instance is complete.
    Complete,
    /// We have reached consensus on a round change.
    RoundChangeConsensus,
}

impl InstanceState {
    /// Returns the payload-free [`InstanceStateKind`] of this state.
    pub fn kind(&self) -> InstanceStateKind {
        match self {
            InstanceState::AwaitingProposal => InstanceStateKind::AwaitingProposal,
            InstanceState::Prepare { .. } => InstanceStateKind::Prepare,
            InstanceState::Commit { .. } => InstanceStateKind::Commit,
            InstanceState::SentRoundChange => InstanceStateKind::SentRoundChange,
            InstanceState::Complete => InstanceStateKind::Complete,
            InstanceState::RoundChangeConsensus => InstanceStateKind::RoundChangeConsensus,
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
