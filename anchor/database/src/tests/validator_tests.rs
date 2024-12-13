use super::test_prelude::*;

#[cfg(test)]
mod validator_database_tests {
    use super::*;

    #[test]
    // Test updating the fee recipient
    fn test_update_fee_recipient() {
        let fixture = TestFixture::new();
        let cluster = &fixture.cluster;
        let new_address = Address::random();

        // Update fee recipient
        fixture
            .db
            .update_fee_recipient(
                cluster.cluster_id,
                cluster.validator_metadata.validator_pubkey.clone(),
                new_address,
            )
            .expect("Failed to update fee recipient");

        // Verify update in memory state
        let metadata = &fixture
            .db
            .get_validator_metadata(&cluster.cluster_id)
            .expect("Failed to get cluster metadata");
        assert_eq!(
            metadata.fee_recipient, new_address,
            "Fee recipient not updated in memory"
        );

        // Verify update in database
        let validator = queries::get_validator(
            &fixture.db,
            &cluster.validator_metadata.validator_pubkey.to_string(),
        )
        .expect("Validator not found in database");
        assert_eq!(
            validator.3,
            new_address.to_string(),
            "Fee recipient not updated in database"
        );
    }

    #[test]
    /// Test updating the graffiti of a validator
    fn test_update_graffiti() {
        let fixture = TestFixture::new();
        let cluster = &fixture.cluster;
        let new_graffiti = Graffiti::default(); // Or create a specific test graffiti

        // Update graffiti
        fixture
            .db
            .update_graffiti(
                cluster.cluster_id,
                cluster.validator_metadata.validator_pubkey.clone(),
                new_graffiti,
            )
            .expect("Failed to update graffiti");

        // Verify update in memory state
        let metadata = &fixture
            .db
            .get_validator_metadata(&cluster.cluster_id)
            .expect("Failed to get cluster metadata");
        assert_eq!(
            metadata.graffiti, new_graffiti,
            "Graffiti not updated in memory"
        );
    }

    #[test]
    /// Test updating the fee recipient of a validator that does not exist
    fn test_update_validator_nonexistent_cluster() {
        let fixture = TestFixture::new();
        let nonexistent_cluster_id = ClusterId(*fixture.cluster.cluster_id + 1);

        let result = fixture.db.update_fee_recipient(
            nonexistent_cluster_id,
            fixture.cluster.validator_metadata.validator_pubkey.clone(),
            Address::random(),
        );

        assert!(
            result.is_err(),
            "Should fail when updating non-existent cluster"
        );
    }
}
