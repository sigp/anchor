use super::test_prelude::*;
use openssl::pkey::Public;
use openssl::rsa::Rsa;
use rand::Rng;
use rusqlite::params;
use std::path::PathBuf;
use tempfile::TempDir;
use types::test_utils::{SeedableRng, TestRandom, XorShiftRng};

const DEFAULT_NUM_OPERATORS: u64 = 4;
const RSA_KEY_SIZE: u32 = 2048;
const DEFAULT_SEED: [u8; 16] = [42; 16];

// Test fixture for common scnearios
#[derive(Debug)]
pub struct TestFixture {
    pub db: NetworkDatabase,
    pub cluster: Cluster,
    pub validator: ValidatorMetadata,
    pub shares: Vec<Share>,
    pub operators: Vec<Operator>,
    pub path: PathBuf,
    pub pubkey: Rsa<Public>,
    _temp_dir: TempDir,
}

impl TestFixture {
    // Generate a database that is populated with a full cluster. This operator is a part of the
    // cluster, so membership data should be saved
    pub fn new() -> Self {
        // generate the operators and pick the first one to be us
        let operators: Vec<Operator> = (0..DEFAULT_NUM_OPERATORS)
            .map(generators::operator::with_id)
            .collect();
        let us = operators
            .first()
            .expect("Failed to get operator")
            .rsa_pubkey
            .clone();

        let temp_dir = TempDir::new().expect("Failed to create temporary directory");
        let db_path = temp_dir.path().join("test.db");
        let db = NetworkDatabase::new(&db_path, &us).expect("Failed to create DB");

        // Insert all of the operators
        operators.iter().for_each(|op| {
            db.insert_operator(op).expect("Failed to insert operator");
        });

        // Build a cluster with all of the operators previously inserted
        let cluster = generators::cluster::with_operators(&operators);

        // Generate one validator that will delegate to this cluster
        let validator = generators::validator::random_metadata(cluster.cluster_id);

        // Generate shares for the validator. Each operator will have one share
        let shares: Vec<Share> = operators
            .iter()
            .map(|op| generators::share::random(cluster.cluster_id, op.id, &validator.public_key))
            .collect();

        db.insert_validator(cluster.clone(), validator.clone(), shares.clone())
            .expect("Failed to insert cluster");

        // End state:
        // There are DEFAULT_NUM_OPERATORS operators in the network
        // There is a single cluster with a single validator
        // The operators acting on behalf of the validator are all of the operators in the network
        // Each operator has a piece of the keyshare for the validator

        Self {
            db,
            cluster,
            operators,
            validator,
            shares,
            path: db_path,
            pubkey: us,
            _temp_dir: temp_dir,
        }
    }

    // Generate an empty database and pick a random public key to be us
    pub fn new_empty() -> Self {
        let temp_dir = TempDir::new().expect("Failed to create temporary directory");
        let db_path = temp_dir.path().join("test.db");
        let pubkey = generators::pubkey::random_rsa();

        let db = NetworkDatabase::new(&db_path, &pubkey).expect("Failed to create test database");
        let cluster = generators::cluster::random(0);

        Self {
            db,
            validator: generators::validator::random_metadata(cluster.cluster_id),
            cluster,
            operators: Vec::new(),
            shares: Vec::new(),
            path: db_path,
            pubkey,
            _temp_dir: temp_dir,
        }
    }
}

// Generator functions for test data
pub mod generators {
    use super::*;

    // Generate a random operator. Either with a specific id or a specific public key
    pub mod operator {
        use super::*;

        pub fn with_id(id: u64) -> Operator {
            let public_key = generators::pubkey::random_rsa();
            Operator::new_with_pubkey(public_key, OperatorId(id), Address::random())
        }
    }

    pub mod cluster {
        use super::*;

        // Generate a random cluster with a specific number of operators
        pub fn random(num_operators: u64) -> Cluster {
            let cluster_id: [u8; 32] = rand::thread_rng().gen();
            let cluster_id = ClusterId(cluster_id);
            let members = (0..num_operators).map(OperatorId).collect();
            let owner_recipient = Address::random();

            Cluster {
                cluster_id,
                owner: owner_recipient,
                fee_recipient: owner_recipient,
                faulty: 0,
                liquidated: false,
                cluster_members: members,
            }
        }

        // Generate a cluster with a specific set of operators
        pub fn with_operators(operators: &[Operator]) -> Cluster {
            let cluster_id: [u8; 32] = rand::thread_rng().gen();
            let cluster_id = ClusterId(cluster_id);
            let members = operators.iter().map(|op| op.id).collect();
            let owner_recipient = Address::random();

            Cluster {
                cluster_id,
                owner: owner_recipient,
                fee_recipient: owner_recipient,
                faulty: 0,
                liquidated: false,
                cluster_members: members,
            }
        }
    }

    pub mod share {
        use super::*;
        // Generate a random keyshare
        pub fn random(cluster_id: ClusterId, operator_id: OperatorId, pk: &PublicKey) -> Share {
            Share {
                validator_pubkey: pk.clone(),
                operator_id,
                cluster_id,
                share_pubkey: pubkey::random(),
                encrypted_private_key: [0u8; 256],
            }
        }
    }

    pub mod pubkey {
        use super::*;

        // Generate a random RSA public key for operators
        pub fn random_rsa() -> Rsa<Public> {
            let priv_key = Rsa::generate(RSA_KEY_SIZE).expect("Failed to generate RSA key");
            priv_key
                .public_key_to_pem()
                .and_then(|pem| Rsa::public_key_from_pem(&pem))
                .expect("Failed to process RSA key")
        }

        // Generate a random public key for validators
        pub fn random() -> PublicKey {
            let rng = &mut XorShiftRng::from_seed(DEFAULT_SEED);
            PublicKey::random_for_test(rng)
        }
    }

    pub mod validator {
        use super::*;

        // Generate random ValidatorMetdata
        // assumes fee_recipient = owner.
        pub fn random_metadata(cluster_id: ClusterId) -> ValidatorMetadata {
            ValidatorMetadata {
                public_key: pubkey::random(),
                cluster_id,
                index: ValidatorIndex(rand::thread_rng().gen_range(0..100)),
                graffiti: Graffiti::default(),
            }
        }
    }
}

// Database queries for testing
// This will extract information corresponding to the original tables
pub mod queries {
    use super::*;
    use std::str::FromStr;

    // Single selection query statements
    const GET_OPERATOR: &str =
        "SELECT operator_id, public_key, owner_address FROM operators WHERE operator_id = ?1";
    const GET_CLUSTER: &str = "SELECT cluster_id, owner, fee_recipient, faulty, liquidated FROM clusters WHERE cluster_id = ?1";
    const GET_SHARES: &str = "SELECT share_pubkey, encrypted_key, cluster_id, operator_id FROM shares WHERE validator_pubkey = ?1";
    const GET_VALIDATOR: &str = "SELECT validator_pubkey, cluster_id, validator_index,  graffiti FROM validators WHERE validator_pubkey = ?1";
    const GET_MEMBERS: &str = "SELECT operator_id FROM cluster_members WHERE cluster_id = ?1";

    // Get an operator from the database
    pub fn get_operator(db: &NetworkDatabase, id: OperatorId) -> Option<Operator> {
        let conn = db.connection().unwrap();
        let mut stmt = conn
            .prepare(GET_OPERATOR)
            .expect("Failed to prepare statement");

        stmt.query_row(params![*id], |row| {
            let operator = Operator::try_from(row).expect("Failed to create operator");
            Ok(operator)
        })
        .ok()
    }

    // Get a Cluster from the database
    pub fn get_cluster(db: &NetworkDatabase, id: ClusterId) -> Option<Cluster> {
        let members = get_cluster_members(db, id)?;
        let conn = db.connection().unwrap();
        let mut stmt = conn
            .prepare(GET_CLUSTER)
            .expect("Failed to prepare statement");

        stmt.query_row(params![*id], |row| {
            let cluster = Cluster::try_from((row, members))?;
            Ok(cluster)
        })
        .ok()
    }

    // Get a share from the database
    pub fn get_shares(db: &NetworkDatabase, pubkey: &PublicKey) -> Option<Vec<Share>> {
        let conn = db.connection().unwrap();
        let mut stmt = conn
            .prepare(GET_SHARES)
            .expect("Failed to prepare statement");
        let shares: Result<Vec<_>, _> = stmt
            .query_map(params![pubkey.to_string()], |row| {
                let share_pubkey_str = row.get::<_, String>(0)?;
                let share_pubkey = PublicKey::from_str(&share_pubkey_str).unwrap();
                let encrypted_private_key: [u8; 256] = row.get(1)?;

                // Get the OperatorId from column 6 and ClusterId from column 1
                let cluster_id = ClusterId(row.get(2)?);
                let operator_id = OperatorId(row.get(3)?);

                Ok(Share {
                    validator_pubkey: pubkey.clone(),
                    operator_id,
                    cluster_id,
                    share_pubkey,
                    encrypted_private_key,
                })
            })
            .ok()?
            .collect();
        match shares {
            Ok(vec) if !vec.is_empty() => Some(vec),
            _ => None,
        }
    }

    // Get a ClusterMember from the database
    fn get_cluster_members(
        db: &NetworkDatabase,
        cluster_id: ClusterId,
    ) -> Option<Vec<ClusterMember>> {
        let conn = db.connection().unwrap();
        let mut stmt = conn
            .prepare(GET_MEMBERS)
            .expect("Failed to prepare statement");
        let members: Result<Vec<_>, _> = stmt
            .query_map([cluster_id.0], |row| {
                Ok(ClusterMember {
                    operator_id: OperatorId(row.get(0)?),
                    cluster_id,
                })
            })
            .ok()?
            .collect();
        match members {
            Ok(vec) if !vec.is_empty() => Some(vec),
            _ => None,
        }
    }

    // Get ValidatorMetadata from the database
    pub fn get_validator(
        db: &NetworkDatabase,
        validator_pubkey: &str,
    ) -> Option<ValidatorMetadata> {
        let conn = db.connection().unwrap();
        let mut stmt = conn
            .prepare(GET_VALIDATOR)
            .expect("Failed to prepare statement");

        stmt.query_row(params![validator_pubkey], |row| {
            let validator = ValidatorMetadata::try_from(row)?;
            Ok(validator)
        })
        .ok()
    }
}

/// Database assertions for testing
pub mod assertions {

    use super::*;

    // Assertions on operator information fetches from in memory and the database
    pub mod operator {
        use super::*;

        // Asserts data between the two operators is the same
        fn data(op1: &Operator, op2: &Operator) {
            // Verify all fields match
            assert_eq!(op1.id, op2.id, "Operator ID mismatch");
            assert_eq!(
                op1.rsa_pubkey.public_key_to_pem().unwrap(),
                op2.rsa_pubkey.public_key_to_pem().unwrap(),
                "Operator public key mismatch"
            );
            assert_eq!(op1.owner, op2.owner, "Operator owner mismatch");
        }

        // Verifies that the operator is in memory
        pub fn exists_in_memory(db: &NetworkDatabase, operator: &Operator) {
            let stored_operator = db
                .get_operator(&operator.id)
                .expect("Operator should exist");
            data(operator, &stored_operator);
        }

        // Verifies that the operator is not in memory
        pub fn exists_not_in_memory(db: &NetworkDatabase, operator: OperatorId) {
            assert!(!db.operator_exists(&operator));
        }

        // Verify that the operator is in the database
        pub fn exists_in_db(db: &NetworkDatabase, operator: &Operator) {
            let db_operator =
                queries::get_operator(db, operator.id).expect("Operator not found in database");
            data(operator, &db_operator);
        }

        // Verify that the operator does not exist in the database
        pub fn exists_not_in_db(db: &NetworkDatabase, operator_id: OperatorId) {
            // Check database
            assert!(
                queries::get_operator(db, operator_id).is_none(),
                "Operator still exists in database"
            );
        }
    }

    // All validator related assertions
    pub mod validator {
        use super::*;

        fn data(v1: &ValidatorMetadata, v2: &ValidatorMetadata) {
            assert_eq!(v1.cluster_id, v2.cluster_id);
            assert_eq!(v1.graffiti, v2.graffiti);
            assert_eq!(v1.index, v2.index);
            assert_eq!(v1.public_key, v2.public_key);
        }
        // Verifies that the cluster is in memory
        pub fn exists_in_memory(db: &NetworkDatabase, v: &ValidatorMetadata) {
            let stored_validator = db
                .metadata()
                .get_by(&v.public_key)
                .expect("Metadata should exist");
            data(v, &stored_validator);
        }

        // Verifies that the cluster is not in memory
        pub fn exists_not_in_memory(db: &NetworkDatabase, v: &ValidatorMetadata) {
            let stored_validator = db.metadata().get_by(&v.public_key);
            assert!(stored_validator.is_none());
        }

        // Verify that the cluster is in the database
        pub fn exists_in_db(db: &NetworkDatabase, v: &ValidatorMetadata) {
            let db_validator = queries::get_validator(db, &v.public_key.to_string())
                .expect("Validator should exist");
            data(v, &db_validator);
        }

        // Verify that the cluster does not exist in the database
        pub fn exists_not_in_db(db: &NetworkDatabase, v: &ValidatorMetadata) {
            let db_validator = queries::get_validator(db, &v.public_key.to_string());
            assert!(db_validator.is_none());
        }
    }

    // Cluster assetions
    pub mod cluster {
        use super::*;
        fn data(c1: &Cluster, c2: &Cluster) {
            assert_eq!(c1.cluster_id, c2.cluster_id);
            assert_eq!(c1.owner, c2.owner);
            assert_eq!(c1.fee_recipient, c2.fee_recipient);
            assert_eq!(c1.faulty, c2.faulty);
            assert_eq!(c1.liquidated, c2.liquidated);
            assert_eq!(c1.cluster_members, c2.cluster_members);
        }
        // Verifies that the cluster is in memory
        pub fn exists_in_memory(db: &NetworkDatabase, c: &Cluster) {
            assert!(db.member_of_cluster(&c.cluster_id));
            let stored_cluster = db
                .clusters()
                .get_by(&c.cluster_id)
                .expect("Cluster should exist");
            data(c, &stored_cluster)
        }

        // Verifies that the cluster is not in memory
        pub fn exists_not_in_memory(db: &NetworkDatabase, cluster_id: ClusterId) {
            assert!(!db.member_of_cluster(&cluster_id));
            let stored_cluster = db.clusters().get_by(&cluster_id);
            assert!(stored_cluster.is_none());
        }

        // Verify that the cluster is in the database
        pub fn exists_in_db(db: &NetworkDatabase, c: &Cluster) {
            let db_cluster =
                queries::get_cluster(db, c.cluster_id).expect("Cluster not found in database");
            data(c, &db_cluster);
        }

        // Verify that the cluster does not exist in the database
        pub fn exists_not_in_db(db: &NetworkDatabase, cluster_id: ClusterId) {
            // Check database
            assert!(
                queries::get_cluster(db, cluster_id).is_none(),
                "Cluster exists in database"
            );
        }
    }

    //
    pub mod share {
        use super::*;
        fn data(s1: &Share, s2: &Share) {
            assert_eq!(s1.cluster_id, s2.cluster_id);
            assert_eq!(s1.encrypted_private_key, s2.encrypted_private_key);
            assert_eq!(s1.operator_id, s2.operator_id);
            assert_eq!(s1.share_pubkey, s2.share_pubkey);
        }

        // Verifies that a share is in memory
        pub fn exists_in_memory(db: &NetworkDatabase, validator_pubkey: &PublicKey, s: &Share) {
            let stored_share = db
                .shares()
                .get_by(validator_pubkey)
                .expect("Share should exist");
            data(s, &stored_share);
        }

        // Verifies that a share is not in memory
        pub fn exists_not_in_memory(db: &NetworkDatabase, validator_pubkey: &PublicKey) {
            let stored_share = db.shares().get_by(validator_pubkey);
            assert!(stored_share.is_none());
        }

        // Verifies that all of the shares for a validator are in the database
        pub fn exists_in_db(db: &NetworkDatabase, validator_pubkey: &PublicKey, s: &[Share]) {
            let db_shares =
                queries::get_shares(db, validator_pubkey).expect("Shares should exist in db");
            // have to pair them up since we dont know what order they will be returned from db in
            db_shares
                .iter()
                .flat_map(|share| {
                    s.iter()
                        .filter(|share2| share.operator_id == share2.operator_id)
                        .map(move |share2| (share, share2))
                })
                .for_each(|(share, share2)| data(share, share2));
        }

        // Verifies that all of the shares for a validator are not in the database
        pub fn exists_not_in_db(db: &NetworkDatabase, validator_pubkey: &PublicKey) {
            let shares = queries::get_shares(db, validator_pubkey);
            assert!(shares.is_none());
        }
    }
}
