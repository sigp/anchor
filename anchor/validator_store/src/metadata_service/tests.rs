use types::{AttestationData, Checkpoint, Epoch, FixedBytesExtended, Hash256, Slot};

use super::calculate_attestation_score;

fn create_attestation_data(source_epoch: u64, target_epoch: u64) -> AttestationData {
    AttestationData {
        slot: Slot::new(0),
        index: 0,
        beacon_block_root: Hash256::zero(),
        source: Checkpoint {
            epoch: Epoch::new(source_epoch),
            root: Hash256::zero(),
        },
        target: Checkpoint {
            epoch: Epoch::new(target_epoch),
            root: Hash256::zero(),
        },
    }
}

#[test]
fn test_scoring_higher_epochs_win() {
    let newer = create_attestation_data(100, 101);
    let newer_result = calculate_attestation_score(&newer, Slot::new(3232), Some(Slot::new(3230)));

    // base = 100 + 101 = 201, distance = 3232 - 3230 = 2, bonus = 1/(1+2) = 0.333...
    // Expected score = 201 + 0.333... = 201.333...
    assert!((newer_result.score - 201.333333).abs() < 0.001);

    let older = create_attestation_data(99, 100);
    let older_result = calculate_attestation_score(&older, Slot::new(3232), Some(Slot::new(3230)));

    // base = 99 + 100 = 199, distance = 2, bonus = 0.333...
    // Expected score = 199.333...
    assert!((older_result.score - 199.333333).abs() < 0.001);

    assert!(newer_result.score > older_result.score);
}

#[test]
fn test_scoring_proximity_bonus() {
    let data = create_attestation_data(100, 101);
    let attestation_slot = Slot::new(3232);

    let result_distance_zero =
        calculate_attestation_score(&data, attestation_slot, Some(Slot::new(3232)));

    // base = 201, distance = 3232 - 3232 = 0, bonus = 1/(1+0) = 1.0
    // Expected score = 201 + 1.0 = 202.0
    assert_eq!(result_distance_zero.score, 202.0);

    let result_distance_one =
        calculate_attestation_score(&data, attestation_slot, Some(Slot::new(3231)));

    // base = 201, distance = 3232 - 3231 = 1, bonus = 1/(1+1) = 0.5
    // Expected score = 201 + 0.5 = 201.5
    assert_eq!(result_distance_one.score, 201.5);

    assert!(result_distance_zero.score > result_distance_one.score);
}

#[test]
fn test_scoring_no_head_slot() {
    let data = create_attestation_data(100, 101);
    let attestation_slot = Slot::new(3232);

    let result = calculate_attestation_score(&data, attestation_slot, None);

    // base = 100 + 101 = 201, no head slot = no bonus
    // Expected score = 201.0
    assert_eq!(result.score, 201.0);
}

#[test]
fn test_scoring_head_after_attestation_slot() {
    let data = create_attestation_data(100, 101);
    let attestation_slot = Slot::new(3230);

    let result = calculate_attestation_score(&data, attestation_slot, Some(Slot::new(3232)));

    // base = 201, head_slot (3232) > attestation_slot (3230), no bonus
    // Expected score = 201.0
    assert_eq!(result.score, 201.0);
}

#[test]
fn test_scoring_same_base_different_proximity() {
    let data = create_attestation_data(100, 101);
    let attestation_slot = Slot::new(3232);

    let result_distance_1 =
        calculate_attestation_score(&data, attestation_slot, Some(Slot::new(3231)));

    // base = 201, distance = 1, bonus = 1/(1+1) = 0.5
    // Expected score = 201.5
    assert_eq!(result_distance_1.score, 201.5);

    let result_distance_2 =
        calculate_attestation_score(&data, attestation_slot, Some(Slot::new(3230)));

    // base = 201, distance = 2, bonus = 1/(1+2) = 0.333...
    // Expected score = 201.333...
    assert!((result_distance_2.score - 201.333333).abs() < 0.001);
}
