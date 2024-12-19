use super::test_prelude::*;
use openssl::pkey::Public;
use openssl::rsa::Rsa;
use rand::Rng;
use rusqlite::{params, OptionalExtension};
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
    pub operators: Vec<Operator>,
    pub path: PathBuf,
    pub pubkey: Rsa<Public>,
    _temp_dir: TempDir,
}

impl TestFixture {
    // Generate a database that is populated with a full cluster. We are a member of the cluster so
    // the in state store will also be populated
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
        operators.iter().for_each(|op| {
            db.insert_operator(op).expect("Failed to insert operator");
        });

        // Build cluster, shares, and validator data
        let cluster = generators::cluster::with_operators(&operators);
        let validator = generators::validator::random_metadata(cluster.cluster_id);
        let shares = operators
            .iter()
            .map(|op| generators::share::random(cluster.cluster_id, op.id))
            .collect();

        db.insert_validator(cluster.clone(), validator, shares)
            .expect("Failed to insert cluster");

        Self {
            db,
            cluster,
            operators,
            path: db_path,
            pubkey: us,
            _temp_dir: temp_dir,
        }
    }

    // Generate an emtpy database and pick a random public key to be us
    pub fn new_empty() -> Self {
        let temp_dir = TempDir::new().expect("Failed to create temporary directory");
        let db_path = temp_dir.path().join("test.db");
        let pubkey = generators::pubkey::random_rsa();

        let db = NetworkDatabase::new(&db_path, &pubkey).expect("Failed to create test database");

        Self {
            db,
            cluster: generators::cluster::random(0),
            operators: Vec::new(),
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

        pub fn with_pubkey(pubkey: Rsa<Public>) -> Operator {
            let id = OperatorId(rand::thread_rng().gen::<u32>().into());
            Operator::new_with_pubkey(pubkey, id, Address::random())
        }

        pub fn with_id(id: u64) -> Operator {
            let public_key = generators::pubkey::random_rsa();
            Operator::new_with_pubkey(public_key, OperatorId(id), Address::random())
        }
    }

    pub mod cluster {
        use super::*;

        // Generate a fully cluster with a configurable number of operators
        // Generate a random cluster
        pub fn random(num_operators: u64) -> Cluster {
            let cluster_id = ClusterId(rand::thread_rng().gen::<u32>().into());
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
            let cluster_id = ClusterId(rand::thread_rng().gen::<u32>().into());
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

    pub mod member {
        use super::*;

        // Generate a new cluster member for a cluster and operator
        pub fn new(cluster_id: ClusterId, operator_id: OperatorId) -> ClusterMember {
            ClusterMember {
                operator_id,
                cluster_id,
            }
        }
    }

    pub mod share {
        use super::*;

        // Generate a random keyshare
        pub fn random(cluster_id: ClusterId, operator_id: OperatorId) -> Share {
            Share {
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

    // Get an operator from the database
    pub fn get_operator(db: &NetworkDatabase, id: OperatorId) -> Option<Operator> {
        let conn = db.connection().unwrap();
        let operators = conn.prepare("SELECT operator_id, public_key, owner_address FROM operators WHERE operator_id = ?1")
            .unwrap()
            .query_row(params![*id], |row| Ok(row.try_into().unwrap()))
            .ok();
        operators
    }

    // Get a Cluster from the database
    pub fn get_cluster(db: &NetworkDatabase, id: ClusterId) -> Option<(i64, i64, bool)> {
        let conn = db.connection().unwrap();

        let cluster = conn
            .prepare("SELECT cluster_id, faulty, liquidated FROM clusters WHERE cluster_id = ?1")
            .unwrap()
            .query_row(params![*id], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .optional()
            .unwrap();
        cluster
    }

    // Get a share from the database
    pub fn get_shares(
        db: &NetworkDatabase,
        cluster_id: ClusterId,
    ) -> Vec<(String, i64, i64, Option<String>)> {
        let conn = db.connection().unwrap();

        let mut stmt = conn
            .prepare("SELECT validator_pubkey, cluster_id, operator_id, share_pubkey FROM shares WHERE cluster_id = ?1")
            .unwrap();
        let shares = stmt
            .query_map(params![*cluster_id], |row| {
                Ok((
                    row.get(0).unwrap(),
                    row.get(1).unwrap(),
                    row.get(2).unwrap(),
                    row.get(3).unwrap(),
                ))
            })
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        shares
    }

    // Get a ClusterMember from the database
    pub fn get_cluster_member(
        db: &NetworkDatabase,
        cluster_id: ClusterId,
        operator_id: OperatorId,
    ) -> Option<Vec<ClusterMember>> {
        let conn = db.connection().expect("Failed to get a DB connection");
        let mut stmt = conn
            .prepare(SQL[&SqlStatement::GetClusterMembers])
            .expect("Failed to prepare statement");
        let members = stmt
            .query_map([cluster_id.0], |row| {
                Ok(ClusterMember {
                    operator_id: OperatorId(row.get(0)?),
                    cluster_id,
                })
            })
            .optional()
            .unwrap();

        members.collect()
    }

    // Get ValidatorMetadata from the database
    pub fn get_validator(
        db: &NetworkDatabase,
        validator_pubkey: &str,
    ) -> Option<(String, i64, String, String, i64)> {
        let conn = db.connection().unwrap();
        let validator = conn.prepare("SELECT validator_pubkey, cluster_id, owner, fee_recipient, validator_index FROM validators WHERE validator_pubkey = ?1")
            .unwrap()
            .query_row(params![validator_pubkey], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)))
            .optional()
            .unwrap();
        validator
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
    }

    /*


        // Verifies that the cluster does not exist in the state store
        pub fn assert_cluster_exists_not_in_store(db: &NetworkDatabase, cluster: &Cluster) {
            // Just make sure we have 0 references to the cluster_id
            db.read_state(|state| {
                let cluster_id = cluster.cluster_id;
                assert!(!state.clusters.contains(&cluster_id));
                assert!(!state.shares.contains_key(&cluster_id));
                assert!(!state.validator_metadata.contains_key(&cluster_id));
                assert!(!state.cluster_members.contains_key(&cluster_id));
                assert!(!state.cluster_members.contains_key(&cluster_id));
            });
        }

        // Verifies that the cluster exists correctly in the state store
        pub fn assert_cluster_exists_in_store(db: &NetworkDatabase, cluster: &Cluster) {
            // - operators: HashMap<OperatorId, Operator>,
            // Verify all operators exist and are cluster members
            db.read_state(|state| {
                let operator_ids: Vec<OperatorId> = cluster
                    .cluster_members
                    .iter()
                    .map(|c| c.operator_id)
                    .collect();

                for id in operator_ids {
                    // Check operator exists
                    assert!(
                        db.operator_exists(&id),
                        "Operator {} not found in database",
                        *id
                    );

                    // - cluster_members: HashMap<ClusterId, HashSet<OperatorId>>,
                    // Check operator is recorded as cluster member
                    assert!(
                        state.cluster_members[&cluster.cluster_id].contains(&id),
                        "Operator {} not recorded as cluster member in memory state",
                        *id
                    );
                }

                // - clusters: HashSet<ClusterId>,
                // Verify cluster is recorded in memory state
                assert!(
                    state.clusters.contains(&cluster.cluster_id),
                    "Cluster ID not found in memory state"
                );

                // - shares: HashMap<ClusterId, Share>,
                // Verify shares exists and share data matches if we're a member
                if let Some(our_id) = state.id {
                    if let Some(our_member) = cluster
                        .cluster_members
                        .iter()
                        .find(|m| m.operator_id == our_id)
                    {
                        let stored_share = state.shares[&cluster.cluster_id].clone();
                        assert_eq!(
                            stored_share.share_pubkey, our_member.share.share_pubkey,
                            "Share public key mismatch"
                        );
                        assert_eq!(
                            stored_share.encrypted_private_key, our_member.share.encrypted_private_key,
                            "Encrypted private key mismatch"
                        );
                    }
                }
                assert!(
                    state.shares.contains_key(&cluster.cluster_id),
                    "No share found for cluster"
                );

                // - validator_metadata: HashMap<ClusterId, ValidatorMetadata>,
                // Verify validator metadata matches
                let validator_metadata = db
                    .get_validator_metadata(&cluster.cluster_id)
                    .expect("Failed to get metadata")
                    .clone();
                assert_eq!(
                    validator_metadata.owner, cluster.validator_metadata.owner,
                    "Validator owner mismatch"
                );
                assert_eq!(
                    validator_metadata.validator_index, cluster.validator_metadata.validator_index,
                    "Validator index mismatch"
                );
                assert_eq!(
                    validator_metadata.fee_recipient, cluster.validator_metadata.fee_recipient,
                    "Fee recipient mismatch"
                );
                assert_eq!(
                    validator_metadata.graffiti, cluster.validator_metadata.graffiti,
                    "Graffiti mismatch"
                );
            });
        }

        // Database (Persistent Storage) Assertions
        // These assertions verify the persistent state in the SQLite dataabase


        // Verifies that a cluster exists in the database
        pub fn assert_cluster_exists_in_db(db: &NetworkDatabase, cluster: &Cluster) {
            // Check cluster base data
            let (id, faulty, liquidated) =
                queries::get_cluster(db, cluster.cluster_id).expect("Cluster not found in database");

            assert_eq!(id as u64, *cluster.cluster_id, "Cluster ID mismatch");
            assert_eq!(
                faulty as u64, cluster.faulty,
                "Cluster faulty count mismatch"
            );
            assert_eq!(
                liquidated, cluster.liquidated,
                "Cluster liquidated status mismatch"
            );

            // Verify cluster members
            for member in &cluster.cluster_members {
                let member_exists =
                    queries::get_cluster_member(db, member.cluster_id, member.operator_id)
                        .expect("Cluster member not found in database");

                assert_eq!(
                    member_exists.0 as u64, *member.cluster_id,
                    "Cluster member cluster ID mismatch"
                );
                assert_eq!(
                    member_exists.1 as u64, *member.operator_id,
                    "Cluster member operator ID mismatch"
                );
            }

            // Verify shares
            let shares = queries::get_shares(db, cluster.cluster_id);
            assert!(!shares.is_empty(), "No shares found for cluster");

            // Verify validator metadata
            let validator =
                queries::get_validator(db, &cluster.validator_metadata.validator_pubkey.to_string())
                    .expect("Validator not found in database");

            assert_eq!(
                validator.0,
                cluster.validator_metadata.validator_pubkey.to_string(),
                "Validator pubkey mismatch"
            );
            assert_eq!(
                validator.1 as u64, *cluster.cluster_id,
                "Validator cluster ID mismatch"
            );
            assert_eq!(
                validator.2,
                cluster.validator_metadata.owner.to_string(),
                "Validator owner mismatch"
            );
        }

        // Verifies that a cluster does not exist in the database
        pub fn assert_cluster_exists_not_in_db(db: &NetworkDatabase, cluster: &Cluster) {
            // Verify cluster base data is gone
            assert!(
                queries::get_cluster(db, cluster.cluster_id).is_none(),
                "Cluster still exists in database"
            );

            // Verify all cluster members are gone
            for member in &cluster.cluster_members {
                assert!(
                    queries::get_cluster_member(db, member.cluster_id, member.operator_id).is_none(),
                    "Cluster member still exists in database"
                );
            }

            // Verify all shares are gone
            let shares = queries::get_shares(db, cluster.cluster_id);
            assert!(shares.is_empty(), "Shares still exist for cluster");

            // Verify validator metadata is gone
            assert!(
                queries::get_validator(db, &cluster.validator_metadata.validator_pubkey.to_string())
                    .is_none(),
                "Validator still exists in database"
            );
        }
    */
}
