#[cfg(test)]
mod validator_database_tests {
    use types::Graffiti;

    use crate::test_utils::{InMemoryTestFixture, assertions};

    #[test]
    /// Test updating the graffiti of a validator
    fn test_update_graffiti() {
        let fixture = InMemoryTestFixture::new();
        let new_graffiti = Graffiti::default();
        let mut validator = fixture.validator.clone();

        let mut conn = fixture.db.connection().unwrap();
        let tx = conn.transaction().unwrap();

        // update the graffiti
        assert!(
            fixture
                .db
                .update_graffiti(&validator.public_key, new_graffiti, &tx)
                .is_ok()
        );

        // confirm that it has changed both in the db and memory
        // exists call will also check data values
        validator.graffiti = new_graffiti;
        assertions::validator::exists_in_db(&validator, &tx);
        assertions::validator::exists_in_memory(&fixture.db, &validator);
    }
}
