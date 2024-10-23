//! A collection of types used by the QBFT modules
use crate::validation::ValidatedData;
use derive_more::{Add, Deref, From};
use std::cmp::Eq;
use std::fmt::Debug;
use std::hash::Hash;

/// Generic LeaderFunction trait to allow for future implementations of the QBFT module
pub trait LeaderFunction {
    /// Returns true if we are the leader
    fn leader_function(
        &self,
        operator_id: &OperatorId,
        round: Round,
        instance_height: InstanceHeight,
        committee_size: usize,
    ) -> bool;
}

#[derive(Debug, Clone)]
pub struct DefaultLeaderFunction {}

impl LeaderFunction for DefaultLeaderFunction {
    fn leader_function(
        &self,
        operator_id: &OperatorId,
        round: Round,
        instance_height: InstanceHeight,
        committee_size: usize,
    ) -> bool {
        *operator_id == ((*round + *instance_height) % committee_size).into()
    }
}

/// This represents an individual round, these change on regular time intervals
#[derive(Clone, Copy, Debug, Deref, Default, Add, PartialEq, Eq, Hash, PartialOrd)]
pub struct Round(usize);

impl Round {
    /// Returns the next round
    pub fn next(&self) -> Round {
        Round(self.0 + 1)
    }

    /// Sets the current round
    pub fn set(&mut self, round: Round) {
        *self = round;
    }
}

/// The operator that is participating in the consensus instance.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash, From, Deref)]
pub struct OperatorId(usize);

/// The instance height behaves like an "ID" for the QBFT instance. It is used to uniquely identify
/// different instances, that have the same operator id.
#[derive(Clone, Copy, Debug, Default)]
pub struct InstanceHeight(usize);

impl Deref for InstanceHeight {
    type Target = usize;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

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
}

/// Generic Data trait to allow for future implementations of the QBFT module
// Messages that can be received from the message_in channel
#[derive(Debug, Clone)]
pub enum InMessage<D: Debug + Clone + Eq + Hash> {
    /// A PROPOSE message to be sent on the network.
    Propose(OperatorId, ConsensusData<D>),
    /// A PREPARE message to be sent on the network.
    Prepare(OperatorId, ConsensusData<D>),
    /// A commit message to be sent on the network.
    Commit(OperatorId, ConsensusData<D>),
    /// Round change message received from network
    RoundChange(OperatorId, Round, Option<ConsensusData<D>>),
}

/// Messages that may be sent to the message_out channel from the instance to the client processor
#[derive(Debug, Clone)]
pub enum OutMessage<D: Debug + Clone + Eq + Hash> {
    /// A PROPOSE message to be sent on the network.
    Propose(ConsensusData<D>),
    /// A PREPARE message to be sent on the network.
    Prepare(ConsensusData<D>),
    /// A commit message to be sent on the network.
    Commit(ConsensusData<D>),
    /// The round has ended, send this message to the network to inform all participants.
    RoundChange(Round, Option<ConsensusData<D>>),
    /// The consensus instance has completed.
    Completed(Completed<D>),
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

impl<D> From<ConsensusData<ValidatedData<D>>> for ConsensusData<D> {
    fn from(value: ConsensusData<ValidatedData<D>>) -> Self {
        ConsensusData {
            round: value.round,
            data: value.data.data,
        }
    }
}

#[derive(Debug, Clone)]
/// The consensus instance has finished.
pub enum Completed<D> {
    /// The instance has timed out.
    TimedOut,
    /// Consensus was reached on the provided data.
    Success(D),
}
