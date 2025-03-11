use client::{Anchor, Client};
use types::MinimalEthSpec;
pub struct LocalAnchorNode {
    // datadir
}

impl LocalAnchorNode {
    pub fn new(index: usize) -> Self {
        let config = Self::testing_anchor_config();
        //Client::run::<MinimalEthSpec>(anchor_executor, config).await
        Self {}
    }

    pub fn testing_anchor_config() -> Anchor {
        // todo!()
        todo!()
    }
}
