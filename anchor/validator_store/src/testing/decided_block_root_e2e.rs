//! End-to-end tests for the decided-root write hook.
//!
//! These drive the real `sign_block` path, so the store is written by production code rather
//! than by a direct helper call. Two invariants need this level: the shape of the stored root
//! (the decoded block's `canonical_root()`, not the QBFT wrapper hash), and the abort ordering
//! on conflict (before threshold signing, not after).
use std::sync::Arc;

use bls::PublicKeyBytes;
use eth2::types::FullBlockContents;
use ssv_types::{
    OperatorId, ValidatorIndex,
    consensus::{
        BEACON_ROLE_PROPOSER, DataVersion, ProposerConsensusData, QbftData, ValidatorDuty,
    },
};
use ssz::Encode;
use ssz_types::VariableList;
use types::{
    BeaconBlock, BeaconBlockElectra, BeaconBlockGloas, ChainSpec, EmptyBlock, ForkName, Hash256,
    MainnetEthSpec, Slot,
};
use validator_store::{UnsignedBlock, ValidatorStore};

use super::common::*;
use crate::{DecidedBlockContext, DecidedBlockKey, Error, SpecificError};

/// Slot the block duty runs at. The harness clock sits at `TEST_SLOT`, and `sign_block` rejects
/// a block whose slot is beyond the current slot.
const DUTY_SLOT: u64 = TEST_SLOT;

fn test_operator_ids() -> [OperatorId; 4] {
    [OperatorId(1), OperatorId(2), OperatorId(3), OperatorId(4)]
}

/// Shared fixture: a harness on the given spec plus the pubkey of its single validator.
struct ValidatorStoreTestState {
    harness: ValidatorStoreTestHarness,
    pubkey: PublicKeyBytes,
}

impl ValidatorStoreTestState {
    fn new(spec: Arc<ChainSpec>) -> ValidatorStoreTestState {
        let committee = create_committee_setup(&test_operator_ids(), 1, 5);
        let harness = ValidatorStoreTestHarness::new_with_options(
            vec![committee],
            OperatorId(1),
            HarnessOptions {
                spec,
                ..Default::default()
            },
        );
        let pubkey = harness.validator_pubkey(0, 0);
        ValidatorStoreTestState { harness, pubkey }
    }

    /// Fixture on a Gloas-at-genesis spec.
    fn gloas() -> ValidatorStoreTestState {
        ValidatorStoreTestState::new(gloas_at_genesis_spec())
    }

    /// Fixture on an Electra-at-genesis spec.
    fn electra() -> ValidatorStoreTestState {
        ValidatorStoreTestState::new(electra_at_genesis_spec())
    }

    /// Builds an empty Gloas block at `DUTY_SLOT`.
    ///
    /// `EmptyBlock::empty` fixes the slot to `spec.genesis_slot`, so the slot is set on the inner
    /// struct before wrapping.
    fn gloas_block(&self) -> BeaconBlock<MainnetEthSpec> {
        let mut block = BeaconBlockGloas::<MainnetEthSpec>::empty(&self.harness.spec);
        block.slot = Slot::new(DUTY_SLOT);
        BeaconBlock::Gloas(block)
    }
}

/// After a Gloas `sign_block`, the stored root is the decoded decided block's `canonical_root()`.
///
/// The second assertion guards the negative: `ProposerConsensusData::hash()` is a plausible but
/// wrong value to store, so the stored root must differ from it.
#[tokio::test(flavor = "multi_thread")]
async fn stored_root_is_the_decoded_block_root_not_the_qbft_wrapper_hash() {
    let validator_store_state = ValidatorStoreTestState::gloas();
    let pubkey = validator_store_state.pubkey;
    let block = validator_store_state.gloas_block();
    let expected_root = block.canonical_root();
    let expected_context = {
        let bid = &block
            .body()
            .signed_execution_payload_bid()
            .expect("a Gloas block carries a bid")
            .message;
        DecidedBlockContext {
            beacon_block_root: expected_root,
            parent_block_root: bid.parent_block_root,
            execution_requests_root: bid.execution_requests_root,
            builder_index: bid.builder_index,
        }
    };

    // The echoing mock decides exactly the value the store proposes, so the wrapper the store
    // builds internally is reproducible here for the negative assertion.
    let wrapper_hash = ProposerConsensusData {
        duty: ValidatorDuty {
            r#type: BEACON_ROLE_PROPOSER,
            pub_key: pubkey,
            slot: Slot::new(DUTY_SLOT),
            validator_index: ValidatorIndex(5),
            committee_index: 0,
            committee_length: 0,
            committees_at_slot: 0,
            validator_committee_index: 0,
            validator_sync_committee_indices: Default::default(),
        },
        version: DataVersion::from(ForkName::Gloas),
        data_ssz: VariableList::new(block.as_ssz_bytes()).expect("block bytes should fit"),
    }
    .hash();

    let signed = validator_store_state
        .harness
        .validator_store
        .sign_block(
            pubkey,
            UnsignedBlock::Full(FullBlockContents::Block(block)),
            Slot::new(DUTY_SLOT),
        )
        .await;

    signed.expect("a Gloas block duty should sign successfully in the harness");
    let stored = validator_store_state
        .harness
        .validator_store
        .decided_block_contexts
        .lock()
        .get(&DecidedBlockKey {
            validator: pubkey,
            slot: Slot::new(DUTY_SLOT),
        })
        .copied()
        .expect("a Gloas block duty must record a decided root");

    assert_eq!(
        stored, expected_context,
        "the stored context must carry the decoded decided block's canonical_root() and bid \
         commitments"
    );
    assert_ne!(
        stored.beacon_block_root, wrapper_hash,
        "the stored root must not be ProposerConsensusData::hash(), the QBFT wrapper hash"
    );
}

/// A conflicting decided root aborts the block duty before any threshold signature is attempted.
///
/// Pre-seeding a different root for `(validator, DUTY_SLOT)` makes the production write conflict.
/// The captured-calls assertion is the load-bearing half: an error alone would also be produced
/// by a hook placed after signing.
#[tokio::test(flavor = "multi_thread")]
async fn conflicting_root_aborts_before_threshold_signing() {
    let validator_store_state = ValidatorStoreTestState::gloas();
    let pubkey = validator_store_state.pubkey;
    let block = validator_store_state.gloas_block();
    validator_store_state
        .harness
        .validator_store
        .record_decided_block_context(
            pubkey,
            Slot::new(DUTY_SLOT),
            DecidedBlockContext {
                beacon_block_root: Hash256::from([0xEE; 32]),
                parent_block_root: Hash256::ZERO,
                execution_requests_root: Hash256::ZERO,
                builder_index: 0,
            },
        )
        .expect("pre-seeding a context into an empty store should succeed");

    let result = validator_store_state
        .harness
        .validator_store
        .sign_block(
            pubkey,
            UnsignedBlock::Full(FullBlockContents::Block(block)),
            Slot::new(DUTY_SLOT),
        )
        .await;

    assert!(
        matches!(
            result,
            Err(Error::SpecificError(SpecificError::DecidedRootConflict(_)))
        ),
        "a conflicting decided root must fail the block duty with DecidedRootConflict, \
         got {:?}",
        result.map(|_| "Ok(SignedBlock)")
    );
    assert!(
        validator_store_state
            .harness
            .captured_calls
            .lock()
            .is_empty(),
        "no threshold signature may be attempted once the decided root conflicts"
    );
}

/// A pre-Gloas block duty creates no store entry. Run on an Electra spec, where the decided
/// value decodes to `UnsignedBlock::Blinded`, so the record branch's full-block arm does not
/// match and the pre-Gloas version gate must not fire either.
#[tokio::test(flavor = "multi_thread")]
async fn pre_gloas_duty_creates_no_entry() {
    let validator_store_state = ValidatorStoreTestState::electra();
    let pubkey = validator_store_state.pubkey;
    let mut electra_block =
        BeaconBlockElectra::<MainnetEthSpec>::empty(&validator_store_state.harness.spec);
    electra_block.slot = Slot::new(DUTY_SLOT);
    let block = BeaconBlock::Electra(electra_block);

    let signed = validator_store_state
        .harness
        .validator_store
        .sign_block(
            pubkey,
            UnsignedBlock::Full(FullBlockContents::Block(block)),
            Slot::new(DUTY_SLOT),
        )
        .await;

    signed.expect("an Electra block duty should sign successfully in the harness");
    assert!(
        validator_store_state
            .harness
            .validator_store
            .decided_block_contexts
            .lock()
            .is_empty(),
        "a pre-Gloas duty must create no decided-root entry"
    );
}
