use client::config::Config;
use std::path::PathBuf;

pub struct LocalAnchorNode {}

impl LocalAnchorNode {
    // Create a new local anchor node
    pub fn new(index: usize, mut anchor_config: Config) -> Self {
        // Access pre-populated database for the operator
        // The data dir for each operator is stored in mock-data/operator-{index}
        let data_path = PathBuf::from("mock-data");
        let data_dir = format!("operator-{}", index);
        anchor_config.data_dir = data_path.join(data_dir);


        Self {}
    }
}
