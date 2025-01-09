use super::test_prelude::*;

#[cfg(test)]
mod validator_database_tests {
    use super::*;

    #[test]
    /// Test updating the graffiti of a validator
    fn test_update_graffiti() {
        let fixture = TestFixture::new();
        let new_graffiti = Graffiti::default();
        let mut validator = fixture.validator;

        // update the graffiti
        assert!(fixture
            .db
            .update_graffiti(&validator.public_key, new_graffiti)
            .is_ok());

        // confirm that it has changed both in the db and memory
        // exists call will also check data values
        validator.graffiti = new_graffiti;
        assertions::validator::exists_in_db(&fixture.db, &validator);
        assertions::validator::exists_in_memory(&fixture.db, &validator);
    }
}
