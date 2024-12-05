use crate::NetworkDatabase;
use rand::Rng;
use rsa::RsaPrivateKey;
use rsa::RsaPublicKey;
use rusqlite::{params, OptionalExtension};
use ssv_types::{
    Cluster, ClusterId, ClusterMember, Operator, OperatorId, Share, ValidatorIndex,
    ValidatorMetadata,
};
use types::test_utils::{SeedableRng, TestRandom, XorShiftRng};
use types::{Address, Graffiti, PublicKey};

// Generate a random PublicKey
pub fn random_pubkey() -> PublicKey {
    let rng = &mut XorShiftRng::from_seed([42; 16]);
    PublicKey::random_for_test(rng)
}

// Generate random operator data
pub fn dummy_operator(id: u64) -> Operator {
    let op_id = OperatorId(id);
    let address = Address::random();
    let _priv_key = RsaPrivateKey::new(&mut rand::thread_rng(), 2048).unwrap();
    let pubkey = RsaPublicKey::from(&_priv_key);
    Operator::new_with_pubkey(pubkey, op_id, address)
}

// Generate a random Cluster
pub fn dummy_cluster(num_operators: u64) -> Cluster {
    let cluster_id = ClusterId(rand::thread_rng().gen::<u32>().into());
    let mut members = Vec::new();

    // Create members for the cluster
    for i in 0..num_operators {
        let member = dummy_cluster_member(cluster_id, OperatorId(i));
        members.push(member);
    }

    Cluster {
        cluster_id,
        cluster_members: members,
        faulty: 0,
        liquidated: false,
        validator_metadata: dummy_validator_metadata(),
    }
}

// Generate a random ClusterMember
pub fn dummy_cluster_member(cluster_id: ClusterId, operator_id: OperatorId) -> ClusterMember {
    ClusterMember {
        operator_id,
        cluster_id,
        share: dummy_share(),
    }
}

// Generate a random Share
pub fn dummy_share() -> Share {
    Share {
        share_pubkey: random_pubkey(),
    }
}

// Generate random validator metadata
pub fn dummy_validator_metadata() -> ValidatorMetadata {
    ValidatorMetadata {
        validator_index: ValidatorIndex(rand::thread_rng().gen::<usize>()),
        validator_pubkey: random_pubkey(),
        fee_recipient: Address::random(),
        graffiti: Graffiti::default(),
        owner: Address::random(),
    }
}

// Get an Operator from the database
pub fn get_operator_from_db(db: &NetworkDatabase, id: OperatorId) -> Option<Operator> {
    let conn = db.connection().unwrap();
    let mut query = conn
        .prepare(
            "SELECT operator_id, public_key, owner_address FROM operators WHERE operator_id = ?1",
        )
        .unwrap();
    let res: Option<(u64, String, String)> = query
        .query_row(params![*id], |row| {
            Ok((
                row.get(0).unwrap(),
                row.get(1).unwrap(),
                row.get(2).unwrap(),
            ))
        })
        .ok();
    res.map(|operator| operator.into())
}

// Get a cluster from the database
pub fn get_cluster_from_db(db: &NetworkDatabase, id: ClusterId) -> Option<(i64, i64, bool)> {
    let conn = db.connection().unwrap();
    let mut stmt = conn
        .prepare("SELECT cluster_id, faulty, liquidated FROM clusters WHERE cluster_id = ?1")
        .unwrap();
    let cluster_row: Option<(i64, i64, bool)> = stmt
        .query_row(params![*id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .optional()
        .unwrap();
    cluster_row
}

// Get a ClusterMember from the database
pub fn get_cluster_member_from_db(
    db: &NetworkDatabase,
    cluster_id: ClusterId,
    operator_id: OperatorId,
) -> Option<(i64, i64)> {
    let conn = db.connection().unwrap();
    let mut stmt = conn.prepare("SELECT cluster_id, operator_id FROM cluster_members WHERE cluster_id = ?1 AND operator_id = ?2").unwrap();
    let member_row: Option<(i64, i64)> = stmt
        .query_row(params![*cluster_id, *operator_id], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .optional()
        .unwrap();
    member_row
}
