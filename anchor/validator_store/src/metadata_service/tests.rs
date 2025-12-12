#[cfg(test)]
mod tests {
    use types::{AttestationData, Checkpoint, Epoch, FixedBytesExtended, Hash256, Slot};

    /// Calculate the score for attestation data based on checkpoint epochs and head slot proximity.
    /// Extracted from fetch_and_score for testing.
    fn calculate_attestation_score(
        attestation_data: &AttestationData,
        attestation_slot: Slot,
        head_slot: Option<Slot>,
    ) -> f64 {
        let base_score = (attestation_data.source.epoch.as_u64()
            + attestation_data.target.epoch.as_u64()) as f64;

        match head_slot {
            Some(head_slot) => {
                let attestation_slot_u64 = attestation_slot.as_u64();
                let head_slot_u64 = head_slot.as_u64();

                if head_slot_u64 <= attestation_slot_u64 {
                    let distance = attestation_slot_u64 - head_slot_u64;
                    let bonus = 1.0 / (1 + distance) as f64;
                    base_score + bonus
                } else {
                    base_score
                }
            }
            None => base_score,
        }
    }

    fn create_test_attestation_data(source_epoch: u64, target_epoch: u64) -> AttestationData {
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
        let newer = create_test_attestation_data(100, 101);
        let older = create_test_attestation_data(99, 100);

        let slot = Slot::new(3232);
        let head = Some(Slot::new(3230));

        let newer_score = calculate_attestation_score(&newer, slot, head);
        let older_score = calculate_attestation_score(&older, slot, head);

        assert!(
            newer_score > older_score,
            "Newer epochs should score higher. newer: {}, older: {}",
            newer_score,
            older_score
        );
    }

    #[test]
    fn test_scoring_proximity_bonus() {
        let data = create_test_attestation_data(100, 101);
        let attestation_slot = Slot::new(3232);

        // Distance 0 (same slot) should get maximum bonus (1.0)
        let distance_zero = Some(Slot::new(3232));
        let score_zero = calculate_attestation_score(&data, attestation_slot, distance_zero);

        // Distance 1 should get smaller bonus (0.5)
        let distance_one = Some(Slot::new(3231));
        let score_one = calculate_attestation_score(&data, attestation_slot, distance_one);

        assert!(
            score_zero > score_one,
            "Distance 0 should score higher than distance 1. distance_0: {}, distance_1: {}",
            score_zero,
            score_one
        );

        // Verify the actual bonus values
        let base_score = 201.0; // 100 + 101
        assert_eq!(
            score_zero,
            base_score + 1.0,
            "Distance 0 should give bonus of 1.0"
        );
        assert_eq!(
            score_one,
            base_score + 0.5,
            "Distance 1 should give bonus of 0.5"
        );
    }

    #[test]
    fn test_scoring_no_head_slot() {
        let data = create_test_attestation_data(100, 101);
        let attestation_slot = Slot::new(3232);

        let score = calculate_attestation_score(&data, attestation_slot, None);

        // Without head slot, should only get base score (no bonus)
        let expected_base_score = 201.0; // 100 + 101
        assert_eq!(
            score, expected_base_score,
            "No head slot should give base score only"
        );
    }

    #[test]
    fn test_scoring_head_after_attestation_slot() {
        let data = create_test_attestation_data(100, 101);
        let attestation_slot = Slot::new(3230);
        let future_head = Some(Slot::new(3232)); // Head is after attestation

        let score = calculate_attestation_score(&data, attestation_slot, future_head);

        // When head is after attestation slot, should not give bonus
        let expected_base_score = 201.0;
        assert_eq!(
            score, expected_base_score,
            "Future head slot should not give bonus"
        );
    }

    #[test]
    fn test_scoring_same_base_different_proximity() {
        let data = create_test_attestation_data(100, 101);
        let attestation_slot = Slot::new(3232);

        // Different head slots with same base score
        let head1 = Some(Slot::new(3231)); // distance 1
        let head2 = Some(Slot::new(3230)); // distance 2

        let score1 = calculate_attestation_score(&data, attestation_slot, head1);
        let score2 = calculate_attestation_score(&data, attestation_slot, head2);

        // Same base, but different proximity bonuses
        let base = 201.0;
        assert_eq!(score1, base + 0.5, "Distance 1 should give bonus of 0.5");
        assert_eq!(
            score2,
            base + 1.0 / 3.0,
            "Distance 2 should give bonus of 1/3"
        );
    }
}
