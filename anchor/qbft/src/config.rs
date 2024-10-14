use super::error::ConfigBuilderError;
use crate::InstanceState;
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct Config<F>
where
    F: LeaderFunction + Clone,
{
    pub operator_id: usize,
    pub instance_height: usize,
    pub round: usize,
    pub state: InstanceState,
    pub pr: usize,
    pub committee_size: usize,
    pub committee_members: Vec<usize>,
    pub quorum_size: usize,
    pub round_time: Duration,
    pub leader_fn: F,
}

/// Generic LeaderFunction trait to allow for future implementations of the QBFT module
pub trait LeaderFunction {
    /// Returns true if we are the leader
    fn leader_function(
        &self,
        operator_id: usize,
        round: usize,
        instance_height: usize,
        committee_size: usize,
    ) -> bool;
}

/*#[derive(Debug, Clone)]
pub enum InstanceState {
    AwaitingProposal,
    Propose,
    Prepare,
    Commit,
    SentRoundChange,
    Complete,
}*/

// TODO: Remove this allow
#[allow(dead_code)]
impl<F: Clone + LeaderFunction> Config<F> {
    /// A unique identification number assigned to the QBFT consensus and given to all members of
    /// the committee
    pub fn operator_id(&self) -> usize {
        self.operator_id
    }
    /// The committee size
    pub fn committee_size(&self) -> usize {
        self.committee_size
    }

    pub fn commmittee_members(&self) -> Vec<usize> {
        self.committee_members.clone()
    }

    /// The quorum size required for the committee to reach consensus
    pub fn quorum_size(&self) -> usize {
        self.quorum_size
    }

    /// The round number -- likely always 0 at initialisation unless we want to implement re-joining an existing
    /// instance that has been dropped locally
    pub fn round(&self) -> usize {
        self.round
    }

    /// How long the round will last
    pub fn round_time(&self) -> Duration {
        self.round_time
    }

    /// Whether the operator is the lead of the committee for the round -- need to properly
    /// implement this in a way that is deterministic based on node IDs
    pub fn leader_fn(&self) -> F {
        // TODO: This clone is bad, we don't want to clone but return a
        // reference. When we generalise this will be changed
        self.leader_fn.clone()
    }
}

impl Default for Config<DefaultLeaderFunction> {
    fn default() -> Self {
        //use the builder to also validate defaults
        ConfigBuilder::default()
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
                operator_id: 0,
                state: InstanceState::AwaitingProposal,
                instance_height: 0,
                committee_size: 5,
                committee_members: vec![0, 1, 2, 3, 4],
                quorum_size: 4,
                round: 0,
                pr: 0,
                round_time: Duration::new(2, 0),
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

// TODO: Remove this lint later, just removes warnings for now
#[allow(dead_code)]
impl<F: LeaderFunction + Clone> ConfigBuilder<F> {
    pub fn operator_id(&mut self, operator_id: usize) -> &mut Self {
        self.config.operator_id = operator_id;
        self
    }

    pub fn instance_height(&mut self, instance_height: usize) -> &mut Self {
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

    pub fn round(&mut self, round: usize) -> &mut Self {
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

// input parameters for leader function need to include the round and the node ID
//
/// TODO: Input will be passed to instance in config by client processor when creating new instance
#[derive(Debug, Clone)]
pub struct DefaultLeaderFunction {}

impl LeaderFunction for DefaultLeaderFunction {
    fn leader_function(
        &self,
        operator_id: usize,
        round: usize,
        instance_height: usize,
        committee_size: usize,
    ) -> bool {
        operator_id == (round + instance_height) % committee_size
    }
}
