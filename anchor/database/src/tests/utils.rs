use crate::NetworkDatabase;
use openssl::rsa::Rsa;
use rand::Rng;
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
    //let pubkey = Rsa::generate(2048).unwrap().public_key_to_pem();
    let _priv_key = Rsa::generate(2048).unwrap();
    let public_key = _priv_key.public_key_to_pem().unwrap();
    let public_key = Rsa::public_key_from_pem(&public_key).unwrap();
    Operator::new_with_pubkey(public_key, op_id, address)
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
        encrypted_private_key: [0u8; 256],
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

// Construct a mock database with a cluster
pub fn db_with_cluster(db: &mut NetworkDatabase) -> Cluster {
    for i in 0..4 {
        let operator = dummy_operator(i);
        db.insert_operator(&operator).unwrap();
    }

    // Insert a dummy cluster
    let cluster = dummy_cluster(4);
    db.insert_cluster(cluster.clone()).unwrap();
    cluster
}

// Get an Operator from the database
pub fn get_operator_from_db(db: &NetworkDatabase, id: OperatorId) -> Option<Operator> {
    let conn = db.connection().unwrap();
    let mut query = conn
        .prepare(
            "SELECT operator_id, public_key, owner_address FROM operators WHERE operator_id = ?1",
        )
        .unwrap();
    let res: Option<Operator> = query
        .query_row(params![*id], |row| Ok(row.try_into().unwrap()))
        .ok();
    res
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

// Get all of the shares for a cluster
// Get all shares for a cluster
pub fn get_shares_from_db(
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

// Get validator metadata from the database
pub fn get_validator_from_db(db: &NetworkDatabase, pubkey: &str) -> Option<(String, i64)> {
    let conn = db.connection().unwrap();
    let mut stmt = conn
        .prepare("SELECT validator_pubkey, cluster_id FROM validators WHERE validator_pubkey = ?1")
        .unwrap();
    stmt.query_row(params![pubkey], |row| Ok((row.get(0)?, row.get(1)?)))
        .optional()
        .unwrap()
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

// Debug print the entire database. For testing purposes
pub fn debug_print_db(db: &NetworkDatabase) {
    let conn = db.connection().unwrap();

    println!("\n=== CLUSTERS ===");
    let mut stmt = conn.prepare("SELECT * FROM clusters").unwrap();
    let clusters = stmt
        .query_map([], |row| {
            Ok(format!(
                "Cluster ID: {}, Faulty: {}, Liquidated: {}",
                row.get::<_, i64>(0).unwrap(),
                row.get::<_, i64>(1).unwrap(),
                row.get::<_, bool>(2).unwrap()
            ))
        })
        .unwrap();
    for cluster in clusters {
        println!("{}", cluster.unwrap());
    }

    println!("\n=== OPERATORS ===");
    let mut stmt = conn.prepare("SELECT * FROM operators").unwrap();
    let operators = stmt
        .query_map([], |row| {
            Ok(format!(
                "Operator ID: {}, PublicKey: {}, Owner: {}",
                row.get::<_, i64>(0).unwrap(),
                row.get::<_, String>(1).unwrap(),
                row.get::<_, String>(2).unwrap()
            ))
        })
        .unwrap();
    for operator in operators {
        println!("{}", operator.unwrap());
    }

    println!("\n=== CLUSTER MEMBERS ===");
    let mut stmt = conn.prepare("SELECT * FROM cluster_members").unwrap();
    let members = stmt
        .query_map([], |row| {
            Ok(format!(
                "Cluster ID: {}, Operator ID: {}",
                row.get::<_, i64>(0).unwrap(),
                row.get::<_, i64>(1).unwrap()
            ))
        })
        .unwrap();
    for member in members {
        println!("{}", member.unwrap());
    }

    println!("\n=== VALIDATORS ===");
    let mut stmt = conn.prepare("SELECT * FROM validators").unwrap();
    let validators = stmt
        .query_map([], |row| {
            Ok(format!(
                "Pubkey: {}, Cluster ID: {}, Fee Recipient: {:?}, Owner: {:?}, Graffiti: {:?}, Index: {:?}",
                row.get::<_, String>(0).unwrap(),
                row.get::<_, i64>(1).unwrap(),
                row.get::<_, Option<String>>(2).unwrap(),
                row.get::<_, Option<String>>(3).unwrap(),
                row.get::<_, Vec<u8>>(4).unwrap(),
                row.get::<_, Option<i64>>(5).unwrap()
            ))
        })
        .unwrap();
    for validator in validators {
        println!("{}", validator.unwrap());
    }

    println!("\n=== SHARES ===");
    let mut stmt = conn.prepare("SELECT * FROM shares").unwrap();
    let shares = stmt
        .query_map([], |row| {
            Ok(format!(
                "Validator Pubkey: {}, Cluster ID: {}, Operator ID: {}, Share Pubkey: {:?}",
                row.get::<_, String>(0).unwrap(),
                row.get::<_, i64>(1).unwrap(),
                row.get::<_, i64>(2).unwrap(),
                row.get::<_, Option<String>>(3).unwrap()
            ))
        })
        .unwrap();
    for share in shares {
        println!("{}", share.unwrap());
    }
}
