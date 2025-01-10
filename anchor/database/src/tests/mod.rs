mod cluster_tests;
mod operator_tests;
mod state_tests;
mod utils;
mod validator_tests;

pub mod test_prelude {
    pub use super::utils::*;
    pub use crate::multi_index::UniqueIndex;
    pub use crate::NetworkDatabase;
    pub use ssv_types::*;
    pub use tempfile::tempdir;
    pub use types::{Address, Graffiti, PublicKey};
}

#[cfg(test)]
mod database_test {
    use super::test_prelude::*;

    #[test]
    fn test_create_database() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("db.sqlite");
        let pubkey = generators::pubkey::random_rsa();
        let db = NetworkDatabase::new(&file, &pubkey);
        assert!(db.is_ok());
    }
}
