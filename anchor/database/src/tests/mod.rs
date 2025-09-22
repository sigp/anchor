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

pub mod test_prelude {
    pub use crate::NetworkDatabase;
}

#[cfg(test)]
mod database_test {
    use tempfile::tempdir;
    use super::test_prelude::*;
    use crate::test_utils::generators;
    use ssv_types::domain_type::DomainType;

    #[test]
    fn test_create_database() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("db.sqlite");
        let pubkey = generators::pubkey::random_rsa();
        let db = NetworkDatabase::new(&file, &pubkey, DomainType::from([0; 4]));
        assert!(db.is_ok());
    }
}
