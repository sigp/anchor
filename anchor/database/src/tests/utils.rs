use super::test_prelude::*;
use openssl::rsa::Rsa;
use rand::Rng;
use rusqlite::{params, OptionalExtension};
use std::path::PathBuf;
use tempfile::TempDir;
use types::test_utils::{SeedableRng, TestRandom, XorShiftRng};
use types::{Address, Graffiti, PublicKey};

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
    _temp_dir: TempDir,
}

impl TestFixture {
    pub fn new(id: Option<u64>) -> Self {
        let temp_dir = TempDir::new().expect("Failed to create temporary directory");
        let db_path = temp_dir.path().join("test.db");
        let mut db = if let Some(id) = id {
            NetworkDatabase::new(&db_path, Some(OperatorId(id)))
                .expect("Failed to create test database")
        } else {
            NetworkDatabase::new(&db_path, None).expect("Failed to create test database")
        };

        let operators: Vec<Operator> = (0..DEFAULT_NUM_OPERATORS)
            .map(generators::operator::with_id)
            .collect();

        operators.iter().for_each(|op| {
            db.insert_operator(op).expect("Failed to insert operator");
        });

        let cluster = generators::cluster::with_operators(&operators);
        db.insert_cluster(cluster.clone())
            .expect("Failed to insert cluster");

        Self {
            db,
            cluster,
            operators,
            path: db_path,
            _temp_dir: temp_dir,
        }
    }

    pub fn new_empty() -> Self {
        let temp_dir = TempDir::new().expect("Failed to create temporary directory");
        let db_path = temp_dir.path().join("test.db");

        let db = NetworkDatabase::new(&db_path, None).expect("Failed to create test database");

        Self {
            db,
            cluster: generators::cluster::random(0),
            operators: Vec::new(),
            path: db_path,
            _temp_dir: temp_dir,
        }
    }
}

// Generator functions for test data
pub mod generators {
    use super::*;

    pub mod operator {
        use super::*;

        pub fn with_id(id: u64) -> Operator {
            let priv_key = Rsa::generate(RSA_KEY_SIZE).expect("Failed to generate RSA key");
            let public_key = priv_key
                .public_key_to_pem()
                .and_then(|pem| Rsa::public_key_from_pem(&pem))
                .expect("Failed to process RSA key");

            Operator::new_with_pubkey(public_key, OperatorId(id), Address::random())
        }
    }

    pub mod cluster {
        use super::*;

        pub fn random(num_operators: u64) -> Cluster {
            let cluster_id = ClusterId(rand::thread_rng().gen::<u32>().into());
            let members = (0..num_operators)
                .map(|i| member::new(cluster_id, OperatorId(i)))
                .collect();

            Cluster {
                cluster_id,
                cluster_members: members,
                faulty: 0,
                liquidated: false,
                validator_metadata: validator::random_metadata(),
            }
        }

        pub fn with_operators(operators: &[Operator]) -> Cluster {
            let cluster_id = ClusterId(rand::thread_rng().gen::<u32>().into());
            let members = operators
                .iter()
                .map(|op| member::new(cluster_id, op.id))
                .collect();

            Cluster {
                cluster_id,
                cluster_members: members,
                faulty: 0,
                liquidated: false,
                validator_metadata: validator::random_metadata(),
            }
        }
    }

    pub mod member {
        use super::*;

        pub fn new(cluster_id: ClusterId, operator_id: OperatorId) -> ClusterMember {
            ClusterMember {
                operator_id,
                cluster_id,
                share: share::random(),
            }
        }
    }

    pub mod share {
        use super::*;

        pub fn random() -> Share {
            Share {
                share_pubkey: pubkey::random(),
                encrypted_private_key: [0u8; 256],
            }
        }
    }

    pub mod pubkey {
        use super::*;

        pub fn random() -> PublicKey {
            let rng = &mut XorShiftRng::from_seed(DEFAULT_SEED);
            PublicKey::random_for_test(rng)
        }
    }

    pub mod validator {
        use super::*;

        pub fn random_metadata() -> ValidatorMetadata {
            ValidatorMetadata {
                validator_index: ValidatorIndex(rand::thread_rng().gen()),
                validator_pubkey: pubkey::random(),
                fee_recipient: Address::random(),
                graffiti: Graffiti::default(),
                owner: Address::random(),
            }
        }
    }
}

/// Database queries for testing
pub mod queries {
    use super::*;

    pub fn get_operator(db: &NetworkDatabase, id: OperatorId) -> Option<Operator> {
        let conn = db.connection().unwrap();
        let operators = conn.prepare("SELECT operator_id, public_key, owner_address FROM operators WHERE operator_id = ?1")
            .unwrap()
            .query_row(params![*id], |row| Ok(row.try_into().unwrap()))
            .ok();
        operators
    }

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

    pub fn get_cluster_member(
        db: &NetworkDatabase,
        cluster_id: ClusterId,
        operator_id: OperatorId,
    ) -> Option<(i64, i64)> {
        let conn = db.connection().unwrap();
        let member = conn.prepare("SELECT cluster_id, operator_id FROM cluster_members WHERE cluster_id = ?1 AND operator_id = ?2")
            .unwrap()
            .query_row(params![*cluster_id, *operator_id], |row| Ok((row.get(0)?, row.get(1)?)))
            .optional()
            .unwrap();
        member
    }

    pub fn get_validator(
        db: &NetworkDatabase,
        validator_pubkey: &str,
    ) -> Option<(String, i64, String)> {
        let conn = db.connection().unwrap();
        let validator = conn.prepare("SELECT validator_pubkey, cluster_id, owner FROM validators WHERE validator_pubkey = ?1")
            .unwrap()
            .query_row(params![validator_pubkey], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .optional()
            .unwrap();
        validator
    }
}

/// Database assertions for testing
pub mod assertions {
    use super::*;

    pub fn assert_operator_exists_fully(db: &NetworkDatabase, operator: &Operator) {
        // Check in-memory state
        let fetched = db
            .get_operator(&operator.id)
            .expect("Operator not found in memory");

        assert_eq!(fetched.id, operator.id, "Operator ID mismatch in memory");
        assert_eq!(
            fetched.rsa_pubkey.public_key_to_pem().unwrap(),
            operator.rsa_pubkey.public_key_to_pem().unwrap(),
            "Operator public key mismatch in memory"
        );
        assert_eq!(
            fetched.owner, operator.owner,
            "Operator owner mismatch in memory"
        );

        // Check database state
        let db_operator =
            queries::get_operator(db, operator.id).expect("Operator not found in database");

        assert_eq!(
            db_operator.rsa_pubkey.public_key_to_pem().unwrap(),
            operator.rsa_pubkey.public_key_to_pem().unwrap(),
            "Operator public key mismatch in database"
        );
        assert_eq!(
            db_operator.id, operator.id,
            "Operator ID mismatch in database"
        );
        assert_eq!(
            db_operator.owner, operator.owner,
            "Operator owner mismatch in database"
        );
    }

    pub fn assert_operator_not_exists_fully(db: &NetworkDatabase, operator_id: OperatorId) {
        // Check memory
        assert!(
            db.get_operator(&operator_id).is_none(),
            "Operator still exists in memory"
        );

        // Check database
        assert!(
            queries::get_operator(db, operator_id).is_none(),
            "Operator still exists in database"
        );
    }

    /// Verifies that a cluster exists and all its data is correctly stored
    pub fn assert_cluster_exists_fully(db: &NetworkDatabase, cluster: &Cluster) {
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

        // Verify cluster is in memory if we're a member
        if let Some(our_id) = db.state.id {
            if cluster
                .cluster_members
                .iter()
                .any(|m| m.operator_id == our_id)
            {
                assert!(
                    db.state.clusters.contains(&cluster.cluster_id),
                    "Cluster not found in memory state"
                );
                assert_eq!(
                    db.state.cluster_members[&cluster.cluster_id].len(),
                    cluster.cluster_members.len(),
                    "Cluster members count mismatch in memory"
                );
            }
        }

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

    /// Verifies that a cluster does not exist in any form
    pub fn assert_cluster_exists_not_fully(db: &NetworkDatabase, cluster: &Cluster) {
        // Verify cluster base data is gone
        assert!(
            queries::get_cluster(db, cluster.cluster_id).is_none(),
            "Cluster still exists in database"
        );

        // Verify cluster is not in memory
        assert!(
            !db.state.clusters.contains(&cluster.cluster_id),
            "Cluster still exists in memory state"
        );
        assert!(
            !db.state.cluster_members.contains_key(&cluster.cluster_id),
            "Cluster members still exist in memory state"
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
        assert!(
            !db.state
                .validator_metadata
                .contains_key(&cluster.cluster_id),
            "Validator metadata still exists in memory state"
        );
    }
}
