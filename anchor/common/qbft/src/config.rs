use super::error::ConfigBuilderError;
use crate::types::{DefaultLeaderFunction, InstanceHeight, LeaderFunction, OperatorId, Round};
use indexmap::IndexSet;
use std::fmt::Debug;
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct Config<F>
where
    F: LeaderFunction + Clone,
{
    operator_id: OperatorId,
    instance_height: InstanceHeight,
    round: Round,
    committee_members: IndexSet<OperatorId>,
    quorum_size: usize,
    round_time: Duration,
    max_rounds: usize,
    leader_fn: F,
}

impl<F: Clone + LeaderFunction> Config<F> {
    /// A unique identification number assigned to the QBFT consensus and given to all members of
    /// the committee
    pub fn operator_id(&self) -> OperatorId {
        self.operator_id
    }

    pub fn instance_height(&self) -> &InstanceHeight {
        &self.instance_height
    }

    /// The round number -- likely always 1 at initialisation unless we want to implement re-joining an existing
    /// instance that has been dropped locally
    pub fn round(&self) -> Round {
        self.round
    }

    pub fn committee_members(&self) -> &IndexSet<OperatorId> {
        &self.committee_members
    }

    pub fn quorum_size(&self) -> usize {
        self.quorum_size
    }

    /// How long the round will last
    pub fn round_time(&self) -> Duration {
        self.round_time
    }

    pub fn max_rounds(&self) -> usize {
        self.max_rounds
    }

    /// Whether the operator is the lead of the committee for the round -- need to properly
    /// implement this in a way that is deterministic based on node IDs
    pub fn leader_fn(&self) -> &F {
        &self.leader_fn
    }

    /// Obtains the maximum number of faulty nodes that this consensus can tolerate
    pub fn get_f(&self) -> usize {
        get_f(self.committee_members.len())
    }

    /// Private constructor so it can only be built by our `ConfigBuilder`.
    fn from_builder(builder: &ConfigBuilder<F>) -> Self {
        Self {
            operator_id: builder.operator_id,
            instance_height: builder.instance_height,
            committee_members: builder.committee_members.clone(),
            round: builder.round,
            round_time: builder.round_time,
            max_rounds: builder.max_rounds,
            quorum_size: builder.quorum_size,
            leader_fn: builder.leader_fn.clone(),
        }
    }
}

fn get_f(members: usize) -> usize {
    (members - 1) / 3
}

/// Builder struct for constructing the QBFT instance configuration
#[derive(Clone, Debug)]
pub struct ConfigBuilder<F = DefaultLeaderFunction>
where
    F: LeaderFunction + Clone,
{
    // Mandatory fields
    operator_id: OperatorId,
    instance_height: InstanceHeight,
    committee_members: IndexSet<OperatorId>,
    leader_fn: F,

    // Optional fields with defaults set in the constructor
    round: Round,
    round_time: Duration,
    max_rounds: usize,
    quorum_size: usize,
}

impl<F> ConfigBuilder<F>
where
    F: LeaderFunction + Clone + Default,
{
    pub fn new(
        operator_id: OperatorId,
        instance_height: InstanceHeight,
        committee_members: IndexSet<OperatorId>,
    ) -> Self {
        let committee_size = committee_members.len();
        let f = get_f(committee_size);
        let default_quorum = committee_size.saturating_sub(f);

        ConfigBuilder {
            operator_id,
            instance_height,
            committee_members,
            round: Round::default(),
            round_time: Duration::new(2, 0),
            max_rounds: 4,
            quorum_size: default_quorum,
            leader_fn: F::default(),
        }
    }
}

impl<F> ConfigBuilder<F>
where
    F: LeaderFunction + Clone,
{
    pub fn new_with_leader_fn(
        operator_id: OperatorId,
        instance_height: InstanceHeight,
        committee_members: IndexSet<OperatorId>,
        leader_fn: F,
    ) -> Self {
        let committee_size = committee_members.len();
        let f = get_f(committee_size);
        let default_quorum = committee_size.saturating_sub(f);

        ConfigBuilder {
            operator_id,
            instance_height,
            committee_members,
            round: Round::default(),
            round_time: Duration::new(2, 0),
            max_rounds: 4,
            quorum_size: default_quorum,
            leader_fn,
        }
    }
    pub fn operator_id(&self) -> OperatorId {
        self.operator_id
    }

    pub fn committee_members(&self) -> &IndexSet<OperatorId> {
        &self.committee_members
    }

    pub fn instance_height(&self) -> &InstanceHeight {
        &self.instance_height
    }

    pub fn round(&self) -> Round {
        self.round
    }

    pub fn round_time(&self) -> Duration {
        self.round_time
    }

    pub fn quorum_size(&self) -> usize {
        self.quorum_size
    }

    pub fn max_rounds(&self) -> usize {
        self.max_rounds
    }

    pub fn leader_fn(&self) -> &F {
        &self.leader_fn
    }

    // Chained setter methods for optional fields to override defaults
    pub fn with_operator_id(mut self, operator_id: OperatorId) -> Self {
        self.operator_id = operator_id;
        self
    }

    pub fn with_instance_height(mut self, instance_height: InstanceHeight) -> Self {
        self.instance_height = instance_height;
        self
    }

    pub fn with_committee_members(mut self, committee_members: IndexSet<OperatorId>) -> Self {
        self.committee_members = committee_members;
        self
    }

    pub fn with_round(mut self, round: Round) -> Self {
        self.round = round;
        self
    }

    pub fn with_round_time(mut self, round_time: Duration) -> Self {
        self.round_time = round_time;
        self
    }

    pub fn with_max_rounds(mut self, max_rounds: usize) -> Self {
        self.max_rounds = max_rounds;
        self
    }

    pub fn with_quorum_size(mut self, quorum_size: usize) -> Self {
        self.quorum_size = quorum_size;
        self
    }

    pub fn with_leader_fn(mut self, leader_fn: F) -> Self {
        self.leader_fn = leader_fn;
        self
    }

    pub fn build(self) -> Result<Config<F>, ConfigBuilderError> {
        // Validate mandatory fields
        let committee_size = self.committee_members.len();
        if committee_size == 0 {
            return Err(ConfigBuilderError::NoParticipants);
        }
        if !self.committee_members.contains(&self.operator_id) {
            return Err(ConfigBuilderError::OperatorNotParticipant);
        }

        // Validate `quorum_size`
        let f = get_f(committee_size);
        if self.quorum_size < f * 2 + 1 || self.quorum_size > committee_size - f {
            return Err(ConfigBuilderError::InvalidQuorumSize);
        }

        // Validate `max_rounds`
        if self.max_rounds == 0 {
            return Err(ConfigBuilderError::ZeroMaxRounds);
        }

        // Validate `round`
        if self.round.get() > self.max_rounds {
            return Err(ConfigBuilderError::ExceedingStartingRound);
        }

        // If everything is okay, build and return a `Config`.
        Ok(Config::from_builder(&self))
    }
}
