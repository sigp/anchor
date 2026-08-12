//! Integration tests for the proposer delay applied by `randao_reveal()`.
//!
//! These pin where the wait sits in the method, not just the delay arithmetic. The
//! hazard defended against is hoisting the wait above `collect_signature`, which would turn
//! the floor into added latency on top of pre-consensus and make the error path block for
//! the full delay.
//!
//! The harness clock is positioned to 100ms into `TEST_SLOT` so the documented recommended
//! delay of 300ms (safely under both config bounds: the 1000ms acknowledgement gate and the
//! 4000ms hard cap) leaves a 200ms remainder.

use std::{sync::Arc, time::Duration};

use bls::PublicKeyBytes;
use slot_clock::ManualSlotClock;
use ssv_types::OperatorId;
use tokio::time::Instant;
use types::{ChainSpec, Epoch, EthSpec, MainnetEthSpec};
use validator_store::ValidatorStore;

use super::common::*;
use crate::{Error, ProposerDelays};

const OUR_OPERATOR_ID: OperatorId = OperatorId(1);
const OPERATOR_IDS: [OperatorId; 4] = [OperatorId(1), OperatorId(2), OperatorId(3), OperatorId(4)];
const COMMITTEE_INDEX: usize = 0;
const VALIDATOR_INDEX: usize = 0;

/// The documented recommended `--proposer-delay-ms` value; a configurable, realistic setting.
const PROPOSER_DELAY: Duration = Duration::from_millis(300);
/// How far into `TEST_SLOT` the clock is repositioned before each test.
const ELAPSED_IN_SLOT_AT_CALL: Duration = Duration::from_millis(100);
/// The floor minus the elapsed time: 300ms - 100ms.
const EXPECTED_REMAINING_WAIT: Duration = Duration::from_millis(200);

/// A `--proposer-delay-epbs-ms` value distinct from [`PROPOSER_DELAY`], so which knob applied is
/// observable from the wait alone. Under the 1000ms hard cap.
const GLOAS_PROPOSER_DELAY: Duration = Duration::from_millis(500);
/// The Gloas-side floor minus the elapsed time: 500ms - 100ms.
const EXPECTED_GLOAS_REMAINING_WAIT: Duration = Duration::from_millis(400);

/// Epoch of `TEST_SLOT` (slot 1) on the mainnet spec.
const SIGNING_EPOCH: Epoch = Epoch::new(0);

/// Places the shared clock `ELAPSED_IN_SLOT_AT_CALL` into `slot`, replacing the harness default
/// of 5s into `TEST_SLOT`, which the sub-second floors could never reach.
fn reposition_clock_early_in_slot(slot_clock: &ManualSlotClock, slot: u64) {
    slot_clock
        .set_current_time(Duration::from_secs(slot * SLOT_DURATION_SECS) + ELAPSED_IN_SLOT_AT_CALL);
}

/// Builds a harness with the given spec and delays, with the clock early in `TEST_SLOT`.
fn harness(spec: Arc<ChainSpec>, proposer_delays: ProposerDelays) -> ValidatorStoreTestHarness {
    let committee = create_committee_setup(&OPERATOR_IDS, 1, 0);
    let harness = ValidatorStoreTestHarness::new_with_options(
        vec![committee],
        OUR_OPERATOR_ID,
        HarnessOptions {
            spec,
            proposer_delays,
            ..Default::default()
        },
    );
    reposition_clock_early_in_slot(&harness.slot_clock, TEST_SLOT);
    harness
}

fn harness_with_proposer_delay() -> ValidatorStoreTestHarness {
    harness(
        Arc::new(ChainSpec::mainnet()),
        ProposerDelays {
            pre_gloas: PROPOSER_DELAY,
            gloas: Duration::ZERO,
        },
    )
}

/// The delay is a floor from slot start, applied *after* signature collection: with the clock
/// 100ms into the slot, the 300ms floor sleeps only the 200ms remainder. Time is paused, so
/// `Instant` observes virtual time and the mock collector's `captured_at` proves the signature
/// was collected before any sleep ran.
#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn randao_reveal_success_waits_only_the_floor_remainder_after_collection() {
    let harness = harness_with_proposer_delay();
    let validator = harness.validator_metadata(COMMITTEE_INDEX, VALIDATOR_INDEX);
    let started = Instant::now();

    let result = harness
        .validator_store
        .randao_reveal(validator.public_key, SIGNING_EPOCH)
        .await;

    result.expect("randao reveal should succeed");
    assert_eq!(
        started.elapsed(),
        EXPECTED_REMAINING_WAIT,
        "delay must be a floor from slot start, sleeping only the remainder"
    );
    let captured = harness.captured_calls.lock();
    assert_eq!(captured.len(), 1, "expected one sign_and_collect call");
    assert_eq!(
        captured[0].captured_at, started,
        "signature must be collected before the proposer delay sleeps"
    );
}

/// A failing duty must not be held to the floor: the wait sits on the success path only, so an
/// unknown pubkey errors immediately with no virtual time spent.
#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn randao_reveal_unknown_pubkey_fails_without_waiting() {
    let harness = harness_with_proposer_delay();
    let unknown_pubkey = PublicKeyBytes::deserialize(&[0xFF; 48]).expect("valid length");
    let started = Instant::now();

    let result = harness
        .validator_store
        .randao_reveal(unknown_pubkey, SIGNING_EPOCH)
        .await;

    assert!(
        matches!(result, Err(Error::UnknownPubkey(pk)) if pk == unknown_pubkey),
        "unknown pubkey should fail with UnknownPubkey"
    );
    assert_eq!(
        started.elapsed(),
        Duration::ZERO,
        "error path must not apply the proposer delay"
    );
}

/// Builds a harness with both delay knobs set to distinct values, under the given spec.
fn harness_with_both_delays(spec: Arc<ChainSpec>) -> ValidatorStoreTestHarness {
    harness(
        spec,
        ProposerDelays {
            pre_gloas: PROPOSER_DELAY,
            gloas: GLOAS_PROPOSER_DELAY,
        },
    )
}

/// From the Gloas fork on, the ePBS value is the floor: with Gloas active at genesis, `TEST_SLOT`
/// is post-fork, so the 500ms ePBS floor sleeps its 400ms remainder rather than the 200ms the
/// pre-Gloas value would have.
#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn randao_reveal_in_gloas_waits_the_gloas_floor_remainder() {
    let harness = harness_with_both_delays(gloas_at_genesis_spec());
    let validator = harness.validator_metadata(COMMITTEE_INDEX, VALIDATOR_INDEX);
    let started = Instant::now();

    let result = harness
        .validator_store
        .randao_reveal(validator.public_key, SIGNING_EPOCH)
        .await;

    result.expect("randao reveal should succeed");
    assert_eq!(
        started.elapsed(),
        EXPECTED_GLOAS_REMAINING_WAIT,
        "a post-Gloas duty must apply the ePBS delay, not the pre-Gloas one"
    );
}

/// Before Gloas the ePBS value must be inert: the values are independent, with no fallback in
/// either direction, so a pre-fork duty waits the pre-Gloas remainder even with the ePBS knob set.
#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn randao_reveal_pre_gloas_ignores_the_gloas_delay() {
    // Mainnet default spec: Gloas is unscheduled, so `TEST_SLOT` is pre-fork.
    let harness = harness_with_both_delays(Arc::new(ChainSpec::mainnet()));
    let validator = harness.validator_metadata(COMMITTEE_INDEX, VALIDATOR_INDEX);
    let started = Instant::now();

    let result = harness
        .validator_store
        .randao_reveal(validator.public_key, SIGNING_EPOCH)
        .await;

    result.expect("randao reveal should succeed");
    assert_eq!(
        started.elapsed(),
        EXPECTED_REMAINING_WAIT,
        "a pre-Gloas duty must apply the pre-Gloas delay, ignoring the ePBS value"
    );
}

/// With Gloas scheduled mid-chain rather than at genesis, a duty in the activation epoch must
/// still pick the ePBS value. This pins that the selection is driven by the duty's actual epoch,
/// not by a genesis-relative constant that the two fork-uniform tests above could not distinguish.
#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn randao_reveal_at_mid_schedule_gloas_activation_waits_the_gloas_floor_remainder() {
    let gloas_epoch = Epoch::new(1);
    let harness = harness_with_both_delays(gloas_at_epoch_spec(gloas_epoch));
    // Reposition to the first slot of the activation epoch, the same 100ms in.
    let first_gloas_slot = gloas_epoch.start_slot(MainnetEthSpec::slots_per_epoch());
    reposition_clock_early_in_slot(&harness.slot_clock, first_gloas_slot.as_u64());
    let validator = harness.validator_metadata(COMMITTEE_INDEX, VALIDATOR_INDEX);
    let started = Instant::now();

    let result = harness
        .validator_store
        .randao_reveal(validator.public_key, gloas_epoch)
        .await;

    result.expect("randao reveal should succeed");
    assert_eq!(
        started.elapsed(),
        EXPECTED_GLOAS_REMAINING_WAIT,
        "a duty in the Gloas activation epoch must apply the ePBS delay"
    );
}
