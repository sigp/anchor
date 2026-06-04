//! Integration test for the Gloas proposer block decide path.

use eth2::types::FullBlockContents;
use ssv_types::OperatorId;
use types::{BeaconBlockGloas, ChainSpec, EmptyBlock, ForkName, MainnetEthSpec, Slot};

use super::common::*;
use crate::UnsignedBlock;

/// Pass a Gloas `BeaconBlock` through `decide_abstract_block`.
/// Observe a `UnsignedBlock::Full(FullBlockContents::Block(_))` carrying the original
/// Gloas-shaped block.
#[tokio::test(flavor = "multi_thread")]
async fn decide_abstract_block_returns_gloas_block_via_full_path() {
    let our_operator_id = OperatorId(1);
    let committee = create_committee_setup(
        &[OperatorId(1), OperatorId(2), OperatorId(3), OperatorId(4)],
        1,
        0,
    );
    let cluster = committee.cluster.clone();
    let validator = committee.validators[0].clone();
    let harness = ValidatorStoreTestHarness::new(vec![committee], our_operator_id);

    let spec = ChainSpec::mainnet();
    let mut block = BeaconBlockGloas::<MainnetEthSpec>::empty(&spec);
    block.slot = Slot::new(TEST_SLOT);
    let block = types::BeaconBlock::Gloas(block);

    let result = harness
        .validator_store
        .decide_abstract_block(&validator, &cluster, &block)
        .await;

    let unsigned_block =
        result.expect("decide_abstract_block should succeed for Gloas BeaconBlock");
    match unsigned_block {
        UnsignedBlock::Full(FullBlockContents::Block(decided_block)) => {
            assert_eq!(
                decided_block.to_ref().fork_name_unchecked(),
                ForkName::Gloas,
                "decided block must carry Gloas fork name"
            );
            assert_eq!(
                decided_block.slot(),
                Slot::new(TEST_SLOT),
                "decided block must preserve the original slot"
            );
        }
        other => panic!(
            "Expected UnsignedBlock::Full(FullBlockContents::Block(_)), got {:?}",
            other
        ),
    }
}
