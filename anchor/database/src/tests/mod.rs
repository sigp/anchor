#[cfg(test)]
mod cluster_tests;
#[cfg(test)]
mod metadata_tests;
#[cfg(test)]
mod operator_tests;
#[cfg(test)]
mod state_tests;
#[cfg(test)]
mod validator_tests;

pub mod utils;

#[cfg(test)]
mod database_test {
    use crate::test_utils::InMemoryTestFixture;

    #[test]
    fn test_create_database() {
        let fixture = InMemoryTestFixture::new_empty();
        assert!(
            fixture.db.state().metadata().length() == 0,
            "Empty database should have no metadata"
        );
    }
}
