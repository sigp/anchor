use client::{Node, Client};
use types::MinimalEthSpec;
use std::path::PathBuf;
pub struct LocalAnchorNode {
    // datadir
}

impl LocalAnchorNode {
    pub fn new(index: usize) -> Self {
        let data_path = PathBuf::from("mock-data");
        let data_dir = format!("operator-{}", index);
        let data_dir = data_path.join(data_dir);
        let config = Self::testing_anchor_config(index);




        //Client::run::<MinimalEthSpec>(anchor_executor, config).await
        Self {}
    }

    pub fn testing_anchor_config(index: usize) -> Node {



        // todo!()
        todo!()
    }
}
