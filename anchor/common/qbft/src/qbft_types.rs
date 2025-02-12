//! A collection of types used by the QBFT modules
use derive_more::{Deref, From};
use indexmap::IndexSet;
use ssv_types::consensus::{QbftMessage, UnsignedSSVMessage};
use ssv_types::message::SignedSSVMessage;
use ssv_types::OperatorId;
use std::cmp::Eq;
use std::fmt::Debug;
use std::hash::Hash;
use std::num::NonZeroUsize;

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

impl WrappedQbftMessage {
    // Validate that the message is well formed
    pub fn validate(&self) -> bool {
        self.signed_message.validate() && self.qbft_message.validate()
    }
}

/// This represents an individual round, these change on regular time intervals
#[derive(Clone, Copy, Debug, Deref, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Round(NonZeroUsize);

impl From<u64> for Round {
    fn from(round: u64) -> Round {
        Round(NonZeroUsize::new(round as usize).expect("round == 0"))
    }
}

impl Default for Round {
    fn default() -> Self {
        // rounds are indexed starting at 1
        Round(NonZeroUsize::new(1).expect("1 != 0"))
    }
}

impl Round {
    /// Returns the next round
    pub fn next(&self) -> Option<Round> {
        self.0.checked_add(1).map(Round)
    }

    /// Sets the current round
    pub fn set(&mut self, round: Round) {
        *self = round;
    }
}

/// The instance height behaves like an "ID" for the QBFT instance. It is used to uniquely identify
/// different instances, that have the same operator id.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash, From, Deref)]
pub struct InstanceHeight(usize);

#[derive(Debug, Clone, Copy)]
#[repr(u8)]
pub enum InstanceState {
    /// Awaiting a propose from a leader
    AwaitingProposal,
    /// Awaiting consensus on PREPARE messages
    Prepare = 1,
    /// Awaiting consensus on COMMIT messages
    Commit,
    /// We have sent a round change message
    SentRoundChange = 4,
    /// The consensus instance is complete
    Complete,
    /// We have reached consensus on a round change
    RoundChangeConsensus,
}

/// Generic Data trait to allow for future implementations of the QBFT module
// Messages that can be received from the message_in channel
#[derive(Debug, Clone)]
pub enum Message {
    /// A PROPOSE message to be sent on the network.
    Propose(OperatorId, UnsignedSSVMessage),
    /// A PREPARE message to be sent on the network.
    Prepare(OperatorId, UnsignedSSVMessage),
    /// A commit message to be sent on the network.
    Commit(OperatorId, UnsignedSSVMessage),
    /// Round change message received from network
    RoundChange(OperatorId, UnsignedSSVMessage),
}

impl Message {
    pub fn desugar(&self) -> (OperatorId, UnsignedSSVMessage) {
        match self {
            Message::Propose(id, msg)
            | Message::Prepare(id, msg)
            | Message::Commit(id, msg)
            | Message::RoundChange(id, msg) => (*id, msg.clone()),
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
