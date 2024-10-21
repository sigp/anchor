use super::error::ConfigBuilderError;
use crate::types::{DefaultLeaderFunction, InstanceHeight, LeaderFunction, OperatorId, Round};
use std::collections::HashSet;
use std::fmt::Debug;
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct Config<F>
where
    F: LeaderFunction + Clone,
{
    pub operator_id: OperatorId,
    pub instance_height: InstanceHeight,
    pub round: Round,
    pub pr: usize,
    pub committee_size: usize,
    pub committee_members: HashSet<OperatorId>,
    pub quorum_size: usize,
    pub round_time: Duration,
    pub max_rounds: usize,
    pub leader_fn: F,
}

impl<F: Clone + LeaderFunction> Config<F> {
    /// A unique identification number assigned to the QBFT consensus and given to all members of
    /// the committee
    pub fn operator_id(&self) -> OperatorId {
        self.operator_id
    }
    /// The committee size
    pub fn committee_size(&self) -> usize {
        self.committee_size
    }

    pub fn commmittee_members(&self) -> HashSet<OperatorId> {
        self.committee_members.clone()
    }

    /// The quorum size required for the committee to reach consensus
    pub fn quorum_size(&self) -> usize {
        self.quorum_size
    }

    /// The round number -- likely always 0 at initialisation unless we want to implement re-joining an existing
    /// instance that has been dropped locally
    pub fn round(&self) -> Round {
        self.round
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
}
impl Default for Config<DefaultLeaderFunction> {
    fn default() -> Self {
        //use the builder to also validate defaults
        ConfigBuilder::<DefaultLeaderFunction>::default()
            .build()
            .expect("Default parameters should be valid")
    }
}
/// Builder struct for constructing the QBFT instance configuration
pub struct ConfigBuilder<F: LeaderFunction + Clone> {
    config: Config<F>,
}

impl Default for ConfigBuilder<DefaultLeaderFunction> {
    fn default() -> Self {
        ConfigBuilder {
            config: Config {
                operator_id: OperatorId::default(),
                instance_height: InstanceHeight::default(),
                committee_size: 0,
                committee_members: HashSet::new(),
                quorum_size: 4,
                round: Round::default(),
                pr: 0,
                round_time: Duration::new(2, 0),
                max_rounds: 4,
                leader_fn: DefaultLeaderFunction {},
            },
        }
    }
}
impl<F: LeaderFunction + Clone> From<Config<F>> for ConfigBuilder<F> {
    fn from(config: Config<F>) -> Self {
        ConfigBuilder { config }
    }
}

impl<F: LeaderFunction + Clone> ConfigBuilder<F> {
    pub fn operator_id(&mut self, operator_id: OperatorId) -> &mut Self {
        self.config.operator_id = operator_id;
        self
    }

    pub fn instance_height(&mut self, instance_height: InstanceHeight) -> &mut Self {
        self.config.instance_height = instance_height;
        self
    }

    pub fn committee_size(&mut self, committee_size: usize) -> &mut Self {
        self.config.committee_size = committee_size;
        self
    }

    pub fn quorum_size(&mut self, quorum_size: usize) -> &mut Self {
        self.config.quorum_size = quorum_size;
        self
    }

    pub fn round(&mut self, round: Round) -> &mut Self {
        self.config.round = round;
        self
    }
    pub fn round_time(&mut self, round_time: Duration) -> &mut Self {
        self.config.round_time = round_time;
        self
    }
    pub fn leader_fn(&mut self, leader_fn: F) -> &mut Self {
        self.config.leader_fn = leader_fn;
        self
    }

    pub fn build(&self) -> Result<Config<F>, ConfigBuilderError> {
        if self.config.quorum_size < 1 {
            return Err(ConfigBuilderError::QuorumSizeTooSmall);
        }

        Ok(self.config.clone())
    }
}
