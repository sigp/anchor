#[cfg(test)]
mod validator_database_tests {
    use bls::PublicKeyBytes;
    use types::Graffiti;

    use crate::{
        PendingStateUpdates,
        test_utils::{InMemoryTestFixture, assertions, commit_and_publish},
    };

    // ==================== Transaction view tests ====================

    /// Ensures `has_own_share_tx` treats "no configured own operator" as "no share".
    ///
    /// This branch is specific to the transaction-view helper and is not exercised by the
    /// integration tests, which all populate the local operator first.
    #[test]
    fn test_has_own_share_tx_no_operator_returns_false() {
        // Arrange: create an empty database with no local operator configured.
        let fixture = InMemoryTestFixture::new_empty();
        let validator_pubkey = PublicKeyBytes::empty();

        let mut conn = fixture.db.connection().unwrap();
        let tx = conn.transaction().unwrap();

        // Act: ask whether the current operator owns a share for an arbitrary validator.
        let has_share = fixture
            .db
            .has_own_share_tx(&validator_pubkey, &tx)
            .expect("Failed to check own share in transaction view");

        // Assert: without a configured own operator, the helper must return false rather than
        // erroring or querying with an invalid operator id.
        assert!(!has_share);
    }

    // ==================== Validator update tests ====================

    #[test]
    /// Test updating the graffiti of a validator
    fn test_update_graffiti() {
        let fixture = InMemoryTestFixture::new();
        let new_graffiti = Graffiti::default();
        let mut validator = fixture.validator.clone();

        let mut conn = fixture.db.connection().unwrap();
        let tx = conn.transaction().unwrap();
        let mut pending = PendingStateUpdates::default();

        // update the graffiti
        assert!(
            fixture
                .db
                .update_graffiti_tx(&validator.public_key, new_graffiti, &tx, &mut pending)
                .is_ok()
        );

        // confirm that it has changed both in the db and memory
        // exists call will also check data values
        validator.graffiti = new_graffiti;
        assertions::validator::exists_in_db(&validator, &tx);
        commit_and_publish(&fixture.db, tx, pending);
        assertions::validator::exists_in_memory(&fixture.db, &validator);
    }
}
