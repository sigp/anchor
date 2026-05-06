use bls::{SecretKey, Signature};
use bls_lagrange::{KeyId, split_with_rng};
use rand::{prelude::*, rngs::StdRng};
use tokio::sync::oneshot;

use super::*;

const TEST_RNG_SEED: u64 = 0xDEAD_BEEF_CAFE_0001;
const TOTAL_SHARES: u64 = 4;
const THRESHOLD: u64 = 3;
const SIGNING_ROOT: Hash256 = Hash256::repeat_byte(0xAB);

fn split_random_master() -> Vec<(OperatorId, SecretKey)> {
    let rng = &mut StdRng::seed_from_u64(TEST_RNG_SEED);
    let master = SecretKey::random();

    split_with_rng(
        &master,
        THRESHOLD,
        (1..=TOTAL_SHARES).map(|x| KeyId::try_from(x).unwrap()),
        rng,
    )
    .expect("split should succeed")
    .into_iter()
    .map(|(kid, sk)| (OperatorId(u64::from(kid)), sk))
    .collect()
}

fn register_notifier(
    state: &mut SignatureCollectorState,
    threshold: u64,
) -> oneshot::Receiver<Arc<Signature>> {
    let (notify, rx) = oneshot::channel();
    let outcome = state.register_request(notify, threshold);
    assert!(
        outcome.is_continue(),
        "register_notifier should not break the collector"
    );
    rx
}

fn feed_partial_sig(
    state: &mut SignatureCollectorState,
    operator_id: OperatorId,
    signature: Signature,
) {
    let outcome = state.add_partial_signature(operator_id, signature);
    assert!(
        outcome.is_continue(),
        "feed_partial_sig should not break the collector"
    );
}

fn expect_signature(rx: &mut oneshot::Receiver<Arc<Signature>>, context: &str) {
    rx.try_recv().expect(context);
}

/// Once `THRESHOLD` valid partial signatures have been fed in, the state
/// reconstructs the master signature and delivers it to a registered notifier.
#[test]
fn state_processes_full_quorum_and_notifies() {
    let shares = split_random_master();
    let mut state = SignatureCollectorState::default();

    let mut result_rx = register_notifier(&mut state, THRESHOLD);

    for (op_id, sk) in &shares[..THRESHOLD as usize] {
        feed_partial_sig(&mut state, *op_id, sk.sign(SIGNING_ROOT));
    }

    expect_signature(
        &mut result_rx,
        "Notifier should receive the reconstructed signature",
    );
}

/// Two `RegisterNotifier` messages disagree on the threshold; the state must
/// `Break`. In production, the recv loop drops the state on `Break`, which
/// drops every queued notifier and surfaces `RecvError` to the callers. The
/// test models that lifetime explicitly via `drop(state)`.
#[test]
fn state_breaks_on_conflicting_thresholds() {
    let mut state = SignatureCollectorState::default();
    let _first_rx = register_notifier(&mut state, THRESHOLD);

    let (second_notify, mut second_rx) = oneshot::channel();
    let outcome = state.register_request(second_notify, THRESHOLD + 1);

    assert!(
        outcome.is_break(),
        "State should Break when a second notifier disagrees on threshold"
    );

    drop(state);

    assert!(
        matches!(
            second_rx.try_recv(),
            Err(oneshot::error::TryRecvError::Closed)
        ),
        "Conflicting notifier sender should be dropped, surfacing RecvError"
    );
}

/// A notifier that registers after reconstruction has already completed should
/// receive the cached signature immediately rather than waiting on a fresh
/// quorum.
#[test]
fn state_delivers_cached_signature_to_late_registrant() {
    let shares = split_random_master();
    let mut state = SignatureCollectorState::default();

    // Reach quorum on the first registrant.
    let mut first_rx = register_notifier(&mut state, THRESHOLD);
    for (op_id, sk) in &shares[..THRESHOLD as usize] {
        feed_partial_sig(&mut state, *op_id, sk.sign(SIGNING_ROOT));
    }
    expect_signature(
        &mut first_rx,
        "First registrant should receive the reconstructed signature",
    );

    // Late registrant arrives after `full_signature` is set.
    let mut late_rx = register_notifier(&mut state, THRESHOLD);
    expect_signature(
        &mut late_rx,
        "Late registrant should receive the cached signature immediately",
    );
}

/// Multiple notifiers registered before quorum should all be notified on a
/// single reconstruction (the `Vec<oneshot::Sender>` fan-out).
#[test]
fn state_notifies_all_registrants_on_reconstruction() {
    let shares = split_random_master();
    let mut state = SignatureCollectorState::default();

    let mut rx_a = register_notifier(&mut state, THRESHOLD);
    let mut rx_b = register_notifier(&mut state, THRESHOLD);
    let mut rx_c = register_notifier(&mut state, THRESHOLD);

    for (op_id, sk) in &shares[..THRESHOLD as usize] {
        feed_partial_sig(&mut state, *op_id, sk.sign(SIGNING_ROOT));
    }

    expect_signature(&mut rx_a, "registrant a should be notified");
    expect_signature(&mut rx_b, "registrant b should be notified");
    expect_signature(&mut rx_c, "registrant c should be notified");
}

/// Partial signatures that arrive before any `RegisterNotifier` is buffered.
/// Reconstruction triggers when a notifier and the threshold arrives.
#[test]
fn state_buffers_shares_arriving_before_first_notifier_register() {
    let shares = split_random_master();
    let mut state = SignatureCollectorState::default();

    for (op_id, sk) in &shares[..THRESHOLD as usize] {
        feed_partial_sig(&mut state, *op_id, sk.sign(SIGNING_ROOT));
    }

    assert!(
        state.full_signature.is_none(),
        "State should not reconstruct without a threshold"
    );

    let mut result_rx = register_notifier(&mut state, THRESHOLD);
    expect_signature(
        &mut result_rx,
        "Notifier should receive the signature reconstructed from buffered shares",
    );
}

/// Partial signatures that arrive after reconstruction are silently dropped:
/// no panic, no Break, no re-buffering of the late share. This is the common
/// case in production: slow operators' shares routinely arrive after a fast
/// majority has already reconstructed the signature.
#[test]
fn state_drops_partial_signatures_after_reconstruction() {
    let shares = split_random_master();
    let mut state = SignatureCollectorState::default();

    let mut rx = register_notifier(&mut state, THRESHOLD);
    for (op_id, sk) in &shares[..THRESHOLD as usize] {
        feed_partial_sig(&mut state, *op_id, sk.sign(SIGNING_ROOT));
    }
    expect_signature(&mut rx, "first registrant should be notified");

    let (late_op, late_sk) = &shares[THRESHOLD as usize];
    feed_partial_sig(&mut state, *late_op, late_sk.sign(SIGNING_ROOT));

    assert!(state.full_signature.is_some(), "cached signature persists");
    assert!(
        state.signature_share.is_empty(),
        "post-reconstruction shares are not buffered"
    );
}
