//! A collection of types used by the QBFT modules
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

// input parameters for leader function need to include the round and the node ID
//
/// TODO: Input will be passed to instance in config by client processor when creating new instance
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
#[derive(Clone, Copy, Debug, Deref, Default, Add, PartialEq, Eq, Hash)]
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

#[derive(Debug, Clone)]
pub enum InstanceState {
    /// We are in this state when we have started a round and are awaiting a proposal from the
    /// leader of this round.
    AwaitingProposal,
    /// We are proposing data for this round.
    Propose,
    Prepare,
    Commit,
    SentRoundChange,
    Complete,
}

/// Generic Data trait to allow for future implementations of the QBFT module
// Messages that can be received from the message_in channel
#[derive(Debug, Clone)]
pub enum InMessage<D: Debug + Clone + Eq + Hash> {
    /// A request for data to form consensus on if we are the leader.
    RecvData(ConsensusData<D>),
    /// A PROPOSE message to be sent on the network.
    Propose(OperatorId, ConsensusData<D>),
    /// A PREPARE message to be sent on the network.
    Prepare(OperatorId, ConsensusData<D>),
    /// A commit message to be sent on the network.
    Commit(OperatorId, ConsensusData<D>),
    /// Round change message received from network
    RoundChange(OperatorId, Round, Option<ConsensusData<D>>),
    /// Close instance message received from the client processor
    RequestClose(usize),
}

/// Messages that may be sent to the message_out channel from the instance to the client processor
#[derive(Debug, Clone)]
pub enum OutMessage<D: Debug + Clone + Eq + Hash> {
    /// A request for data to form consensus on if we are the leader.
    GetData(Round),
    /// A PROPOSE message to be sent on the network.
    Propose(ConsensusData<D>),
    /// A PREPARE message to be sent on the network.
    Prepare(ConsensusData<D>),
    /// A commit message to be sent on the network.
    Commit(ConsensusData<D>),
    /// The round has ended, send this message to the network to inform all participants.
    RoundChange(Round, Option<ConsensusData<D>>),
    /// The consensus instance has completed.
    Completed(Round, Completed<D>),
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
