#[cfg(test)]
mod share_pubkey_tests {
    use std::collections::HashMap;

    use bls::PublicKeyBytes;
    use ssv_types::{OperatorId, ValidatorIndex};

    use crate::test_utils::{InMemoryTestFixture, generators};

    /// Index well outside the fixture's random range (0..100)
    const NON_EXISTENT_VALIDATOR_INDEX: ValidatorIndex = ValidatorIndex(999_999);

    fn assert_share_pubkeys(
        result: &HashMap<OperatorId, PublicKeyBytes>,
        fixture: &InMemoryTestFixture,
    ) {
        assert_eq!(result.len(), fixture.shares.len());
        for share in &fixture.shares {
            let pubkey = result
                .get(&share.operator_id)
                .expect("operator should be present in result");
            assert_eq!(*pubkey, share.share_pubkey);
        }
    }

    #[test]
    fn test_get_share_pubkeys() {
        // Arrange
        let fixture = InMemoryTestFixture::new();
        let validator_index = fixture
            .validator
            .index
            .expect("fixture validator should have an index");

        // Act
        let by_pubkey = fixture
            .db
            .get_share_pubkeys_for_validator(&fixture.validator.public_key)
            .expect("query by pubkey should succeed");
        let by_index = fixture
            .db
            .get_share_pubkeys_for_validator_index(validator_index)
            .expect("query by index should succeed");

        // Assert
        assert_share_pubkeys(&by_pubkey, &fixture);
        assert_share_pubkeys(&by_index, &fixture);
    }

    #[test]
    fn test_get_share_pubkeys_empty() {
        // Arrange
        let fixture = InMemoryTestFixture::new_empty();

        // Act
        let by_pubkey = fixture
            .db
            .get_share_pubkeys_for_validator(&generators::pubkey::random())
            .expect("query by pubkey should succeed on empty db");
        let by_index = fixture
            .db
            .get_share_pubkeys_for_validator_index(NON_EXISTENT_VALIDATOR_INDEX)
            .expect("query by index should succeed on empty db");

        // Assert
        assert!(by_pubkey.is_empty());
        assert!(by_index.is_empty());
    }
}
