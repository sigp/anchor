use super::error::ConfigBuilderError;
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct Config {
    pub instance_id: usize,
    //TODO: need to implement share information - op ID, val pub key, committee info, -- maybe
    //local_id: usize;
    //committee_ids: Vec<usize>;
    //include in config?,
    pub quorum_size: usize,
    pub round: usize,
    pub round_time: Duration,
    pub leader_fn: bool,
}

// I think we need to include the share struct information in the config?

impl Config {
    /// A unique identification number assigned to the QBFT consensus and given to all members of
    /// the committee
    pub fn instance_id(&self) -> usize {
        self.instance_id
    }

    /// The quorum size required for the committee to reach consensus
    pub fn quorum_size(&self) -> usize {
        self.quorum_size
    }

    /// The round number -- likely always 0 unless we want to implement re-joining an existing
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
    pub fn leader_fn(&self) -> bool {
        self.leader_fn
    }
}

impl Default for Config {
    fn default() -> Self {
        //use the builder to also validate defaults
        ConfigBuilder::default()
            .build()
            .expect("Default parameters should be valid")
    }
}
/// Builder struct for constructing the QBFT instance configuration
pub struct ConfigBuilder {
    config: Config,
}

impl Default for ConfigBuilder {
    fn default() -> Self {
        ConfigBuilder {
            config: Config {
                instance_id: 1,
                quorum_size: 2,
                round: 3,
                round_time: Duration::new(2, 0),
                leader_fn: true,
            },
        }
    }
}
impl From<Config> for ConfigBuilder {
    fn from(config: Config) -> Self {
        ConfigBuilder { config }
    }
}

impl ConfigBuilder {
    pub fn instance_id(&mut self, instance_id: usize) -> &mut Self {
        self.config.instance_id = instance_id;
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
    pub fn leader_fn(&mut self, leader_fn: bool) -> &mut Self {
        self.config.leader_fn = leader_fn;
        self
    }

    pub fn build(&self) -> Result<Config, ConfigBuilderError> {
        if self.config.quorum_size < 1 {
            return Err(ConfigBuilderError::QuorumSizeTooSmall);
        }

        Ok(self.config.clone())
    }
}
