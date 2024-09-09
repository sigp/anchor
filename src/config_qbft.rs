use crate::error::ConfigBuilderError;
use std::time::Duration;

#[derive(Clone)]
pub struct InstanceConfig {
    instance_id: usize,
    //TODO: need to implement share information - op ID, val pub key, committee info, -- maybe
    //include in config?,
    quorum_size: usize,
    round: usize,
    round_time: Duration,
    leader_fn: bool,
}

// I think we need to include the share struct information in the config?

impl InstanceConfig {
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

impl Default for InstanceConfig {
    fn default() -> Self {
        //use the builder to also validate defaults
        ConfigBuilder::default()
            .build()
            .expect("Default parameters should be valid")
    }
}
/// Builder struct for constructing the QBFT instance configuration
pub struct ConfigBuilder {
    config: InstanceConfig,
}

impl Default for ConfigBuilder {
    fn default() -> Self {
        ConfigBuilder {
            config: InstanceConfig {
                instance_id: 1,
                quorum_size: 2,
                round: 3,
                round_time: Duration::new(2, 0),
                leader_fn: true,
            },
        }
    }
}
impl From<InstanceConfig> for ConfigBuilder {
    fn from(config: InstanceConfig) -> Self {
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

    pub fn build(&self) -> Result<InstanceConfig, ConfigBuilderError> {
        if self.config.quorum_size < 1 {
            return Err(ConfigBuilderError::QuorumSizeTooSmall);
        }

        Ok(self.config.clone())
    }
}

impl std::fmt::Debug for InstanceConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut builder = f.debug_struct("QBFTInstanceConfig");

        let _ = builder.field("instance_id", &self.instance_id);
        let _ = builder.field("quorum_size", &self.quorum_size);
        let _ = builder.field("round", &self.round);
        let _ = builder.field("round_time", &self.round_time);
        let _ = builder.field("leader_fn", &self.leader_fn);

        builder.finish()
    }
}
