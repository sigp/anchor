use super::test_prelude::*;

#[cfg(test)]
mod validator_database_tests {
    use super::*;
    use types::Address;

    #[test]
    /// Test updating the fee recipient address
    fn test_update_fee_recipient() {
        let mut fixture = TestFixture::new(Some(1));

        let validator_pubkey = fixture.cluster.validator_metadata.validator_pubkey;
        let updated_fee_recipient = Address::random();
        let cluster_id = fixture.cluster.cluster_id;
        fixture
            .db
            .update_fee_recipient(cluster_id, validator_pubkey.clone(), updated_fee_recipient)
            .expect("Failed to update fee recipient");

        // make sure the state store has changed, then check the db
        assert_eq!(
            updated_fee_recipient,
            fixture
                .db
                .get_fee_recipient(&cluster_id)
                .expect("Failed to get fee recipient")
        );
        assert_eq!(
            updated_fee_recipient.to_string(),
            queries::get_validator(&fixture.db, &(validator_pubkey.to_string()))
                .expect("Failed to fetch Validator")
                .3
        );
    }

    #[test]
    /// Test setting the validator index
    fn test_set_validator_index() {
        let mut fixture = TestFixture::new(Some(1));

        let validator_pubkey = fixture.cluster.validator_metadata.validator_pubkey;
        let updated_validator_index = ValidatorIndex(10);
        let cluster_id = fixture.cluster.cluster_id;
        fixture
            .db
            .set_validator_index(
                cluster_id,
                validator_pubkey.clone(),
                updated_validator_index,
            )
            .expect("Failed to update validator index");

        // make sure the state store has changed, then check the db
        assert_eq!(
            updated_validator_index,
            fixture
                .db
                .get_validator_index(&cluster_id)
                .expect("Failed to get validator index")
        );
        assert_eq!(
            *updated_validator_index as i64,
            queries::get_validator(&fixture.db, &(validator_pubkey.to_string()))
                .expect("Failed to fetch Validator")
                .4
        );
    }
}
