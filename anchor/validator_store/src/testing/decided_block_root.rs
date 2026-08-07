//! Tests for the decided-block-root handoff store.
//!
//! These tests call the private `record_decided_block_root` / `get_decided_block_root` helpers
//! directly. The behavior under test is the store's own contract: first-write-wins, read-side
//! staleness, and slot-bounded eviction.
use ssv_types::OperatorId;
use types::{Hash256, Slot};

use super::common::*;
use crate::{DecidedBlockRootKey, Error, MAX_DECIDED_ROOT_AGE_SLOTS, SpecificError};

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

/// A root recorded for `(validator, slot)` reads back unchanged.
#[tokio::test]
async fn record_then_read_returns_the_recorded_root() {
    let harness = harness_with_validators(1);
    let pubkey = harness.validator_pubkey(0, 0);
    let root = Hash256::from([0xAB; 32]);
    set_clock_to_slot(&harness, RECORD_SLOT);

    harness
        .validator_store
        .record_decided_block_root(pubkey, Slot::new(RECORD_SLOT), root)
        .expect("recording a root into an empty store should succeed");
    let read = harness
        .validator_store
        .get_decided_block_root(pubkey, Slot::new(RECORD_SLOT));

    assert_eq!(
        read.expect("a root recorded at the current slot should be readable"),
        root,
        "the store must return the exact root that was recorded"
    );
}

/// A read for a `(validator, slot)` that was never recorded is an error, never a default root.
#[tokio::test]
async fn missing_root_is_unavailable_not_a_default() {
    // Arrange
    let harness = harness_with_validators(1);
    let pubkey = harness.validator_pubkey(0, 0);
    set_clock_to_slot(&harness, RECORD_SLOT);

    // Act
    let read = harness
        .validator_store
        .get_decided_block_root(pubkey, Slot::new(RECORD_SLOT));

    // Assert
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
    // Arrange
    let harness = harness_with_validators(2);
    let recorded_for = harness.validator_pubkey(0, 0);
    let other_validator = harness.validator_pubkey(0, 1);
    let root = Hash256::from([0xAB; 32]);
    set_clock_to_slot(&harness, RECORD_SLOT);
    harness
        .validator_store
        .record_decided_block_root(recorded_for, Slot::new(RECORD_SLOT), root)
        .expect("recording a root into an empty store should succeed");

    // Act
    let read = harness
        .validator_store
        .get_decided_block_root(other_validator, Slot::new(RECORD_SLOT));

    // Assert
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
    // Arrange
    let harness = harness_with_validators(1);
    let pubkey = harness.validator_pubkey(0, 0);
    let root = Hash256::from([0xAB; 32]);
    set_clock_to_slot(&harness, RECORD_SLOT);
    harness
        .validator_store
        .record_decided_block_root(pubkey, Slot::new(RECORD_SLOT), root)
        .expect("recording a root into an empty store should succeed");

    // Act: the next slot is inside the age window, so a hit here would be a keying bug, not
    // a staleness result.
    let read = harness
        .validator_store
        .get_decided_block_root(pubkey, Slot::new(RECORD_SLOT + 1));

    // Assert
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
    // Arrange
    let harness = harness_with_validators(1);
    let pubkey = harness.validator_pubkey(0, 0);
    let root = Hash256::from([0xAB; 32]);
    set_clock_to_slot(&harness, RECORD_SLOT);
    harness
        .validator_store
        .record_decided_block_root(pubkey, Slot::new(RECORD_SLOT), root)
        .expect("recording a root into an empty store should succeed");

    // Act
    let second_write =
        harness
            .validator_store
            .record_decided_block_root(pubkey, Slot::new(RECORD_SLOT), root);

    // Assert
    assert!(
        second_write.is_ok(),
        "re-recording an identical root must be idempotent, got {second_write:?}"
    );
    assert_eq!(
        harness
            .validator_store
            .get_decided_block_root(pubkey, Slot::new(RECORD_SLOT))
            .expect("the root must still be readable after an idempotent rewrite"),
        root,
        "an idempotent rewrite must leave the stored root unchanged"
    );
}

/// A second, different root for one `(validator, slot)` is a hard error, and the first root
/// survives.
#[tokio::test]
async fn conflicting_root_errors_and_keeps_the_first_root() {
    // Arrange
    let harness = harness_with_validators(1);
    let pubkey = harness.validator_pubkey(0, 0);
    let first_root = Hash256::from([0xAB; 32]);
    let conflicting_root = Hash256::from([0xCD; 32]);
    set_clock_to_slot(&harness, RECORD_SLOT);
    harness
        .validator_store
        .record_decided_block_root(pubkey, Slot::new(RECORD_SLOT), first_root)
        .expect("recording a root into an empty store should succeed");

    // Act
    let conflict = harness.validator_store.record_decided_block_root(
        pubkey,
        Slot::new(RECORD_SLOT),
        conflicting_root,
    );

    // Assert
    match conflict {
        Err(SpecificError::DecidedRootConflict(data)) => {
            assert_eq!(
                data.existing_root, first_root,
                "the conflict error must report the first root as the existing one"
            );
            assert_eq!(
                data.new_root, conflicting_root,
                "the conflict error must report the rejected root as the new one"
            );
        }
        other => panic!("a conflicting root must be DecidedRootConflict, got {other:?}"),
    }

    assert_eq!(
        harness
            .validator_store
            .get_decided_block_root(pubkey, Slot::new(RECORD_SLOT))
            .expect("the first root must remain readable after a rejected conflicting write"),
        first_root,
        "a rejected conflicting write must not replace the first root"
    );
}

/// A stale read is rejected while the entry is still in the map. With no later insert, nothing
/// evicts the entry, so only the read-side clock check can reject.
#[tokio::test]
async fn stale_read_is_rejected_while_the_entry_is_still_present() {
    // Arrange
    let harness = harness_with_validators(1);
    let pubkey = harness.validator_pubkey(0, 0);
    let root = Hash256::from([0xAB; 32]);
    set_clock_to_slot(&harness, RECORD_SLOT);
    harness
        .validator_store
        .record_decided_block_root(pubkey, Slot::new(RECORD_SLOT), root)
        .expect("recording a root into an empty store should succeed");

    // Move the clock one slot past the age window. No further insert happens, so no eviction runs.
    set_clock_to_slot(&harness, RECORD_SLOT + MAX_DECIDED_ROOT_AGE_SLOTS + 1);

    // Act
    let read = harness
        .validator_store
        .get_decided_block_root(pubkey, Slot::new(RECORD_SLOT));

    // Assert
    assert!(
        matches!(
            read,
            Err(Error::SpecificError(SpecificError::DecidedRootStale { .. }))
        ),
        "a read past the age window must be DecidedRootStale, got {read:?}"
    );
    assert!(
        harness
            .validator_store
            .decided_block_roots
            .lock()
            .contains_key(&DecidedBlockRootKey {
                validator: pubkey,
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
    // Arrange
    let harness = harness_with_validators(1);
    let pubkey = harness.validator_pubkey(0, 0);
    let root = Hash256::from([0xAB; 32]);
    set_clock_to_slot(&harness, RECORD_SLOT);
    harness
        .validator_store
        .record_decided_block_root(pubkey, Slot::new(RECORD_SLOT), root)
        .expect("recording a root into an empty store should succeed");
    set_clock_to_slot(&harness, RECORD_SLOT + MAX_DECIDED_ROOT_AGE_SLOTS);

    // Act
    let read = harness
        .validator_store
        .get_decided_block_root(pubkey, Slot::new(RECORD_SLOT));

    // Assert
    assert_eq!(
        read.expect("an entry at exactly the max age must still be served"),
        root,
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
    // Arrange
    let harness = harness_with_validators(1);
    let pubkey = harness.validator_pubkey(0, 0);
    let old_slot = Slot::new(RECORD_SLOT);
    let new_slot = Slot::new(RECORD_SLOT + MAX_DECIDED_ROOT_AGE_SLOTS + 1);
    set_clock_to_slot(&harness, RECORD_SLOT);
    harness
        .validator_store
        .record_decided_block_root(pubkey, old_slot, Hash256::from([0xAB; 32]))
        .expect("recording a root into an empty store should succeed");

    // Act
    harness
        .validator_store
        .record_decided_block_root(pubkey, new_slot, Hash256::from([0xCD; 32]))
        .expect("recording a root at a later slot should succeed");

    // Assert
    let stored = harness.validator_store.decided_block_roots.lock();
    assert!(
        !stored.contains_key(&DecidedBlockRootKey {
            validator: pubkey,
            slot: old_slot,
        }),
        "an insert past the age window must evict the older entry"
    );
    assert!(
        stored.contains_key(&DecidedBlockRootKey {
            validator: pubkey,
            slot: new_slot,
        }),
        "the newly inserted entry must survive its own eviction pass"
    );
}

/// A late write for an old slot must not evict a newer live root. Eviction is referenced to the
/// inserted slot, so for a small inserted slot every newer key survives.
#[tokio::test]
async fn out_of_order_old_insert_does_not_disturb_a_newer_root() {
    // Arrange
    let harness = harness_with_validators(1);
    let pubkey = harness.validator_pubkey(0, 0);
    let new_slot = Slot::new(RECORD_SLOT + MAX_DECIDED_ROOT_AGE_SLOTS + 1);
    let newer_root = Hash256::from([0xCD; 32]);
    set_clock_to_slot(&harness, new_slot.as_u64());
    harness
        .validator_store
        .record_decided_block_root(pubkey, new_slot, newer_root)
        .expect("recording a root into an empty store should succeed");

    // Act: a write for a much older slot arrives late.
    harness
        .validator_store
        .record_decided_block_root(pubkey, Slot::new(RECORD_SLOT), Hash256::from([0xAB; 32]))
        .expect("a late write for an older slot should still succeed");

    // Assert
    assert_eq!(
        harness
            .validator_store
            .get_decided_block_root(pubkey, new_slot)
            .expect("the newer root must survive an out-of-order older insert"),
        newer_root,
        "an out-of-order older insert must not evict or alter a newer live root"
    );
}
