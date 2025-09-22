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
    pub use ssv_types::{domain_type::DomainType, *};
    pub use tempfile::tempdir;
    pub use types::{Address, Graffiti, PublicKeyBytes};

    pub use super::utils::*;
    pub use crate::{NetworkDatabase, multi_index::UniqueIndex};
}

#[cfg(test)]
mod database_test {
    use super::test_prelude::*;

    #[test]
    fn test_create_database() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("db.sqlite");
        let pubkey = generators::pubkey::random_rsa();
        let db = NetworkDatabase::new(&file, &pubkey, DomainType::from([0; 4]));
        assert!(db.is_ok());
    }
}
