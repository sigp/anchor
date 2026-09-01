//! Tests for the decided-block-root handoff store.
//!
//! These tests call the private `record_decided_block_context` / `get_decided_block_context`
//! helpers directly. The behavior under test is the store's own contract: first-write-wins,
//! read-side staleness, and slot-bounded eviction.
use bls::PublicKeyBytes;
use ssv_types::OperatorId;
use types::{ExecutionBlockHash, Hash256, Slot};

use super::common::*;
use crate::{
    DecidedBlockContext, DecidedBlockKey, Error, MAX_DECIDED_ROOT_AGE_SLOTS, SpecificError,
};

/// Builds a context whose four fields all derive distinctly from `seed`, so a test that
/// passes with any field dropped or swapped would fail on equality.
fn test_context(seed: u8) -> DecidedBlockContext {
    DecidedBlockContext {
        beacon_block_root: Hash256::from([seed; 32]),
        parent_block_root: Hash256::from([seed.wrapping_add(1); 32]),
        execution_requests_root: Hash256::from([seed.wrapping_add(2); 32]),
        builder_index: seed as u64,
        block_hash: ExecutionBlockHash::from_root(Hash256::from([seed.wrapping_add(3); 32])),
        built_locally: false,
    }
}

/// Distinct from `TEST_SLOT`, so a test cannot pass against a store that ignores the slot.
const RECORD_SLOT: u64 = 10;

fn test_operator_ids() -> [OperatorId; 4] {
    [OperatorId(1), OperatorId(2), OperatorId(3), OperatorId(4)]
}

/// Builds a harness with one committee of `num_validators` validators.
fn harness_with_validators(num_validators: usize) -> ValidatorStoreTestHarness {
    let committee = create_committee_setup(&test_operator_ids(), num_validators, 5);
    ValidatorStoreTestHarness::new(vec![committee], OperatorId(1))
}

/// Places the harness clock at the start of `slot`, so `slot_clock.now()` returns `slot`.
fn set_clock_to_slot(harness: &ValidatorStoreTestHarness, slot: u64) {
    harness.slot_clock.set_slot(slot);
}

/// Shared fixture: the harness plus the validator and roots most tests use.
struct ValidatorStoreTestState {
    harness: ValidatorStoreTestHarness,
    pubkey: PublicKeyBytes,
    context: DecidedBlockContext,
    /// A second, distinct context for conflict and eviction tests.
    other_context: DecidedBlockContext,
}

impl ValidatorStoreTestState {
    fn new(num_validators: usize) -> ValidatorStoreTestState {
        let harness = harness_with_validators(num_validators);
        let pubkey = harness.validator_pubkey(0, 0);
        ValidatorStoreTestState {
            harness,
            pubkey,
            context: test_context(0xAB),
            other_context: test_context(0xCD),
        }
    }
}

/// A root recorded for `(validator, slot)` reads back unchanged.
#[tokio::test]
async fn record_then_read_returns_the_recorded_root() {
    let validator_store_state = ValidatorStoreTestState::new(1);
    set_clock_to_slot(&validator_store_state.harness, RECORD_SLOT);

    validator_store_state
        .harness
        .validator_store
        .record_decided_block_context(
            validator_store_state.pubkey,
            Slot::new(RECORD_SLOT),
            validator_store_state.context,
        )
        .expect("recording a root into an empty store should succeed");
    let read = validator_store_state
        .harness
        .validator_store
        .get_decided_block_context(validator_store_state.pubkey, Slot::new(RECORD_SLOT));

    assert_eq!(
        read.expect("a root recorded at the current slot should be readable"),
        validator_store_state.context,
        "the store must return the exact root that was recorded"
    );
}

/// A read for a `(validator, slot)` that was never recorded is an error, never a default root.
#[tokio::test]
async fn missing_root_is_unavailable_not_a_default() {
    let validator_store_state = ValidatorStoreTestState::new(1);
    set_clock_to_slot(&validator_store_state.harness, RECORD_SLOT);

    let read = validator_store_state
        .harness
        .validator_store
        .get_decided_block_context(validator_store_state.pubkey, Slot::new(RECORD_SLOT));

    assert!(
        matches!(
            read,
            Err(Error::SpecificError(
                SpecificError::DecidedRootUnavailable { .. }
            ))
        ),
        "an unrecorded (validator, slot) must be DecidedRootUnavailable, got {read:?}"
    );
}

/// A root recorded for one validator is not readable under another validator's key.
#[tokio::test]
async fn roots_are_isolated_across_validators() {
    let validator_store_state = ValidatorStoreTestState::new(2);
    let other_validator = validator_store_state.harness.validator_pubkey(0, 1);
    set_clock_to_slot(&validator_store_state.harness, RECORD_SLOT);
    validator_store_state
        .harness
        .validator_store
        .record_decided_block_context(
            validator_store_state.pubkey,
            Slot::new(RECORD_SLOT),
            validator_store_state.context,
        )
        .expect("recording a root into an empty store should succeed");

    let read = validator_store_state
        .harness
        .validator_store
        .get_decided_block_context(other_validator, Slot::new(RECORD_SLOT));

    assert!(
        matches!(
            read,
            Err(Error::SpecificError(
                SpecificError::DecidedRootUnavailable { .. }
            ))
        ),
        "a root recorded for one validator must not be readable for another, got {read:?}"
    );
}

/// A root recorded at one slot is not readable at a neighboring slot, even for the same validator.
#[tokio::test]
async fn roots_are_isolated_across_slots() {
    let validator_store_state = ValidatorStoreTestState::new(1);
    set_clock_to_slot(&validator_store_state.harness, RECORD_SLOT);
    validator_store_state
        .harness
        .validator_store
        .record_decided_block_context(
            validator_store_state.pubkey,
            Slot::new(RECORD_SLOT),
            validator_store_state.context,
        )
        .expect("recording a root into an empty store should succeed");

    // Next slot is inside the age window, so a hit here would be a keying bug, not
    // a staleness result.
    let read = validator_store_state
        .harness
        .validator_store
        .get_decided_block_context(validator_store_state.pubkey, Slot::new(RECORD_SLOT + 1));

    assert!(
        matches!(
            read,
            Err(Error::SpecificError(
                SpecificError::DecidedRootUnavailable { .. }
            ))
        ),
        "a root recorded at one slot must not be readable at another slot, got {read:?}"
    );
}

/// Re-recording the same root is idempotent.
#[tokio::test]
async fn same_root_rewrite_is_idempotent() {
    let validator_store_state = ValidatorStoreTestState::new(1);
    set_clock_to_slot(&validator_store_state.harness, RECORD_SLOT);
    validator_store_state
        .harness
        .validator_store
        .record_decided_block_context(
            validator_store_state.pubkey,
            Slot::new(RECORD_SLOT),
            validator_store_state.context,
        )
        .expect("recording a root into an empty store should succeed");

    let second_write = validator_store_state
        .harness
        .validator_store
        .record_decided_block_context(
            validator_store_state.pubkey,
            Slot::new(RECORD_SLOT),
            validator_store_state.context,
        );

    assert!(
        second_write.is_ok(),
        "re-recording an identical root must be idempotent, got {second_write:?}"
    );
    assert_eq!(
        validator_store_state
            .harness
            .validator_store
            .get_decided_block_context(validator_store_state.pubkey, Slot::new(RECORD_SLOT))
            .expect("the root must still be readable after an idempotent rewrite"),
        validator_store_state.context,
        "an idempotent rewrite must leave the stored root unchanged"
    );
}

/// A second, different root for one `(validator, slot)` is a hard error, and the first root
/// survives.
#[tokio::test]
async fn conflicting_root_errors_and_keeps_the_first_root() {
    let validator_store_state = ValidatorStoreTestState::new(1);
    set_clock_to_slot(&validator_store_state.harness, RECORD_SLOT);
    validator_store_state
        .harness
        .validator_store
        .record_decided_block_context(
            validator_store_state.pubkey,
            Slot::new(RECORD_SLOT),
            validator_store_state.context,
        )
        .expect("recording a root into an empty store should succeed");

    let conflict = validator_store_state
        .harness
        .validator_store
        .record_decided_block_context(
            validator_store_state.pubkey,
            Slot::new(RECORD_SLOT),
            validator_store_state.other_context,
        );

    match conflict {
        Err(SpecificError::DecidedRootConflict(data)) => {
            assert_eq!(
                data.existing_root, validator_store_state.context.beacon_block_root,
                "the conflict error must report the first root as the existing one"
            );
            assert_eq!(
                data.new_root, validator_store_state.other_context.beacon_block_root,
                "the conflict error must report the rejected root as the new one"
            );
        }
        other => panic!("a conflicting root must be DecidedRootConflict, got {other:?}"),
    }

    assert_eq!(
        validator_store_state
            .harness
            .validator_store
            .get_decided_block_context(validator_store_state.pubkey, Slot::new(RECORD_SLOT))
            .expect("the first root must remain readable after a rejected conflicting write"),
        validator_store_state.context,
        "a rejected conflicting write must not replace the first root"
    );
}

/// `built_locally` is excluded from conflict identity and merged with OR: a repeat record
/// with the same decision bindings but a different bit is non-conflicting and finishes true,
/// in both orders.
#[tokio::test]
async fn built_locally_merges_with_or_in_both_orders() {
    for (first_bit, second_bit) in [(true, false), (false, true)] {
        let validator_store_state = ValidatorStoreTestState::new(1);
        set_clock_to_slot(&validator_store_state.harness, RECORD_SLOT);
        let mut first = validator_store_state.context;
        first.built_locally = first_bit;
        let mut second = validator_store_state.context;
        second.built_locally = second_bit;

        validator_store_state
            .harness
            .validator_store
            .record_decided_block_context(
                validator_store_state.pubkey,
                Slot::new(RECORD_SLOT),
                first,
            )
            .expect("first record should succeed");
        validator_store_state
            .harness
            .validator_store
            .record_decided_block_context(
                validator_store_state.pubkey,
                Slot::new(RECORD_SLOT),
                second,
            )
            .expect("a repeat with the same decision bindings must not conflict on the bit");

        let stored = validator_store_state
            .harness
            .validator_store
            .get_decided_block_context(validator_store_state.pubkey, Slot::new(RECORD_SLOT))
            .expect("the context must be readable");
        assert!(
            stored.built_locally,
            "built_locally must merge with OR (order {first_bit}/{second_bit})"
        );
    }
}

/// A stale read is rejected while the entry is still in the map. With no later insert, nothing
/// evicts the entry, so only the read-side clock check can reject.
#[tokio::test]
async fn stale_read_is_rejected_while_the_entry_is_still_present() {
    let validator_store_state = ValidatorStoreTestState::new(1);
    set_clock_to_slot(&validator_store_state.harness, RECORD_SLOT);
    validator_store_state
        .harness
        .validator_store
        .record_decided_block_context(
            validator_store_state.pubkey,
            Slot::new(RECORD_SLOT),
            validator_store_state.context,
        )
        .expect("recording a root into an empty store should succeed");

    // Move the clock one slot past the age window. No further insert happens, so no eviction runs.
    set_clock_to_slot(
        &validator_store_state.harness,
        RECORD_SLOT + MAX_DECIDED_ROOT_AGE_SLOTS + 1,
    );

    let read = validator_store_state
        .harness
        .validator_store
        .get_decided_block_context(validator_store_state.pubkey, Slot::new(RECORD_SLOT));

    assert!(
        matches!(
            read,
            Err(Error::SpecificError(SpecificError::DecidedRootStale { .. }))
        ),
        "a read past the age window must be DecidedRootStale, got {read:?}"
    );
    assert!(
        validator_store_state
            .harness
            .validator_store
            .decided_block_contexts
            .lock()
            .contains_key(&DecidedBlockKey {
                validator: validator_store_state.pubkey,
                slot: Slot::new(RECORD_SLOT),
            }),
        "the entry must still be present, proving the read-side clock check rejected it \
         rather than eviction having removed it"
    );
}

/// An entry at exactly `MAX_DECIDED_ROOT_AGE_SLOTS` of age is still served. The age window is
/// inclusive at its edge.
#[tokio::test]
async fn read_at_exactly_the_max_age_is_served() {
    let validator_store_state = ValidatorStoreTestState::new(1);
    set_clock_to_slot(&validator_store_state.harness, RECORD_SLOT);
    validator_store_state
        .harness
        .validator_store
        .record_decided_block_context(
            validator_store_state.pubkey,
            Slot::new(RECORD_SLOT),
            validator_store_state.context,
        )
        .expect("recording a root into an empty store should succeed");
    set_clock_to_slot(
        &validator_store_state.harness,
        RECORD_SLOT + MAX_DECIDED_ROOT_AGE_SLOTS,
    );

    let read = validator_store_state
        .harness
        .validator_store
        .get_decided_block_context(validator_store_state.pubkey, Slot::new(RECORD_SLOT));

    assert_eq!(
        read.expect("an entry at exactly the max age must still be served"),
        validator_store_state.context,
        "the age window is inclusive at MAX_DECIDED_ROOT_AGE_SLOTS"
    );
}

// No clock-failure test. The harness clock has genesis at time zero, so `now()` cannot return
// `None` here. Reaching the `SlotClock` arm would need a harness genesis change or a new mock
// clock type. Neither is worth it for one `ok_or`.

/// An insert far past an existing entry drops the old entry, bounding the map. Asserted on the
/// map directly, because a stale getter result would pass even if the entry were never evicted.
#[tokio::test]
async fn insert_evicts_entries_older_than_the_age_window() {
    let validator_store_state = ValidatorStoreTestState::new(1);
    let old_slot = Slot::new(RECORD_SLOT);
    let new_slot = Slot::new(RECORD_SLOT + MAX_DECIDED_ROOT_AGE_SLOTS + 1);
    set_clock_to_slot(&validator_store_state.harness, RECORD_SLOT);
    validator_store_state
        .harness
        .validator_store
        .record_decided_block_context(
            validator_store_state.pubkey,
            old_slot,
            validator_store_state.context,
        )
        .expect("recording a root into an empty store should succeed");

    validator_store_state
        .harness
        .validator_store
        .record_decided_block_context(
            validator_store_state.pubkey,
            new_slot,
            validator_store_state.other_context,
        )
        .expect("recording a root at a later slot should succeed");

    let stored = validator_store_state
        .harness
        .validator_store
        .decided_block_contexts
        .lock();
    assert!(
        !stored.contains_key(&DecidedBlockKey {
            validator: validator_store_state.pubkey,
            slot: old_slot,
        }),
        "an insert past the age window must evict the older entry"
    );
    assert!(
        stored.contains_key(&DecidedBlockKey {
            validator: validator_store_state.pubkey,
            slot: new_slot,
        }),
        "the newly inserted entry must survive its own eviction pass"
    );
}

/// An insert at exactly `MAX_DECIDED_ROOT_AGE_SLOTS` past an entry keeps that entry. The
/// eviction window matches the read window at the maximum slot number, so an insert cannot drop a
/// root that a read at the same slot can still serve.
#[tokio::test]
async fn insert_keeps_entries_at_exactly_the_maximum_number_window() {
    let validator_store_state = ValidatorStoreTestState::new(1);
    let old_slot = Slot::new(RECORD_SLOT);
    let maximum_slot_number = Slot::new(RECORD_SLOT + MAX_DECIDED_ROOT_AGE_SLOTS);
    set_clock_to_slot(&validator_store_state.harness, RECORD_SLOT);
    validator_store_state
        .harness
        .validator_store
        .record_decided_block_context(
            validator_store_state.pubkey,
            old_slot,
            validator_store_state.context,
        )
        .expect("recording a root into an empty store should succeed");

    // Insert at exactly the edge of the number window.
    validator_store_state
        .harness
        .validator_store
        .record_decided_block_context(
            validator_store_state.pubkey,
            maximum_slot_number,
            validator_store_state.other_context,
        )
        .expect("recording a root at the maximum slot number should succeed");

    assert!(
        validator_store_state
            .harness
            .validator_store
            .decided_block_contexts
            .lock()
            .contains_key(&DecidedBlockKey {
                validator: validator_store_state.pubkey,
                slot: old_slot,
            }),
        "an insert at exactly the maximum slot number must keep the older entry"
    );
}

/// A late write for an old slot must not evict a newer live root. Eviction is referenced to the
/// inserted slot, so for a small inserted slot every newer key survives.
#[tokio::test]
async fn out_of_order_old_insert_does_not_disturb_a_newer_root() {
    let validator_store_state = ValidatorStoreTestState::new(1);
    let new_slot = Slot::new(RECORD_SLOT + MAX_DECIDED_ROOT_AGE_SLOTS + 1);
    let newer_root = validator_store_state.other_context;
    set_clock_to_slot(&validator_store_state.harness, new_slot.as_u64());
    validator_store_state
        .harness
        .validator_store
        .record_decided_block_context(validator_store_state.pubkey, new_slot, newer_root)
        .expect("recording a root into an empty store should succeed");

    // A write for a much older slot arrives late.
    validator_store_state
        .harness
        .validator_store
        .record_decided_block_context(
            validator_store_state.pubkey,
            Slot::new(RECORD_SLOT),
            validator_store_state.context,
        )
        .expect("a late write for an older slot should still succeed");

    assert_eq!(
        validator_store_state
            .harness
            .validator_store
            .get_decided_block_context(validator_store_state.pubkey, new_slot)
            .expect("the newer root must survive an out-of-order older insert"),
        newer_root,
        "an out-of-order older insert must not evict or alter a newer live root"
    );
}
