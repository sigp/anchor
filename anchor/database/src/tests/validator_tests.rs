#[cfg(test)]
mod validator_database_tests {
    use std::collections::HashMap;

    use ssv_types::ValidatorIndex;
    use types::Graffiti;

    use crate::{
        multi_index::UniqueIndex,
        test_utils::{InMemoryTestFixture, assertions, generators},
    };

    #[test]
    /// Test updating the graffiti of a validator
    fn test_update_graffiti() {
        let fixture = InMemoryTestFixture::new();
        let new_graffiti = Graffiti::default();
        let mut validator = fixture.validator.clone();

        // update the graffiti
        assert!(
            fixture
                .db
                .update_graffiti(&validator.public_key, new_graffiti)
                .is_ok()
        );

        // confirm that it has changed both in the db and memory
        // exists call will also check data values
        validator.graffiti = new_graffiti;
        assertions::validator::exists_in_db(&fixture.db, &validator);
        assertions::validator::exists_in_memory(&fixture.db, &validator);
    }

    #[test]
    /// `set_validator_indices` should update the durable validator row and the post-commit
    /// in-memory metadata view in one call.
    fn test_set_validator_indices_updates_db_and_memory() {
        let fixture = InMemoryTestFixture::new();
        let new_index = ValidatorIndex(42);

        fixture
            .db
            .set_validator_indices(HashMap::from([(fixture.validator.public_key, new_index)]))
            .expect("Validator index update should succeed");

        let mut validator = fixture.validator.clone();
        validator.index = Some(new_index);
        assertions::validator::exists_in_db(&fixture.db, &validator);
        assertions::validator::exists_in_memory(&fixture.db, &validator);
    }

    #[test]
    /// Updating indices for unknown validators is currently an expected no-op at the DB boundary:
    /// SQLite updates zero rows and the in-memory view stays unchanged.
    fn test_set_validator_indices_ignores_unknown_validators() {
        let fixture = InMemoryTestFixture::new();
        let unknown_pubkey =
            generators::validator::random_metadata(fixture.cluster.cluster_id).public_key;

        fixture
            .db
            .set_validator_indices(HashMap::from([(unknown_pubkey, ValidatorIndex(7))]))
            .expect("Unknown validator index updates should not fail");

        let state = fixture.db.state();
        let stored_validator = state
            .metadata()
            .get_by(&fixture.validator.public_key)
            .expect("Original validator should still exist");
        assert_eq!(
            stored_validator.index, fixture.validator.index,
            "Known validator state should remain unchanged",
        );
    }
}
