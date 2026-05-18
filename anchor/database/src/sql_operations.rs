// Metadata
pub const INSERT_METADATA: &str = r#"
    INSERT INTO metadata (
        schema_version,
        domain_type,
        network_name,
        block_number,
        max_operator_id_seen
    )
    SELECT 4, 0, ?1, 0, 0
    WHERE NOT EXISTS (SELECT 1 FROM metadata)
"#;
pub const GET_LEGACY_BLOCK: &str = r#"SELECT * FROM block"#;
pub const GET_MAX_OPERATOR_ID_SEEN: &str = r#"SELECT max_operator_id_seen FROM metadata"#;
pub const SET_MAX_OPERATOR_ID_SEEN: &str = r#"UPDATE metadata SET max_operator_id_seen = ?1"#;

// Operator
pub const INSERT_OPERATOR: &str = r#"
    INSERT INTO operators 
        (operator_id, public_key, owner_address)
    VALUES
        (?1, ?2, ?3)
"#;
pub const INSERT_SKIPPED_OPERATOR_ADD: &str = r#"
    INSERT INTO skipped_operator_adds
        (operator_id, reason)
    VALUES
        (?1, ?2)
    ON CONFLICT (operator_id) DO UPDATE SET reason = excluded.reason
"#;
pub const DELETE_SKIPPED_OPERATOR_ADD: &str =
    r#"DELETE FROM skipped_operator_adds WHERE operator_id = ?1"#;
#[cfg(any(test, feature = "test-utils"))]
pub const GET_SKIPPED_OPERATOR_ADD_REASON: &str =
    r#"SELECT reason FROM skipped_operator_adds WHERE operator_id = ?1"#;
pub const MARK_OPERATOR_REMOVED: &str =
    r#"UPDATE operators SET removed = TRUE WHERE operator_id = ?1"#;
pub const DELETE_OPERATOR: &str = r#"DELETE FROM operators WHERE operator_id = ?1"#;
pub const GET_OPERATOR_STATUS: &str = r#"SELECT removed FROM operators WHERE operator_id = ?1"#;
pub const GET_OPERATOR_ID: &str =
    r#"SELECT operator_id FROM operators WHERE public_key = ?1 AND removed = FALSE"#;
pub const GET_ANY_OPERATOR_ID: &str =
    r#"SELECT operator_id FROM operators WHERE public_key = ?1 LIMIT 1"#;
pub const GET_OPERATOR_KEY: &str =
    r#"SELECT public_key FROM operators WHERE operator_id = ?1 AND removed = FALSE"#;
pub const GET_ALL_OPERATORS: &str = r#"SELECT * FROM operators WHERE removed = FALSE"#;

// Cluster
pub const INSERT_CLUSTER: &str = r#"
    INSERT OR IGNORE INTO clusters 
        (cluster_id, owner) 
    VALUES 
        (?1, ?2)
"#;
pub const INSERT_CLUSTER_MEMBER: &str = r#"
    INSERT OR IGNORE INTO cluster_members
        (cluster_id, operator_id) 
    VALUES 
        (?1, ?2)
"#;
pub const UPDATE_CLUSTER_STATUS: &str = r#"
    UPDATE clusters 
    SET liquidated = ?1 
    WHERE cluster_id = ?2
"#;
pub const GET_ALL_CLUSTERS: &str = r#"
    SELECT DISTINCT
        c.cluster_id,
        c.owner,
        o.fee_recipient,
        c.liquidated
    FROM clusters c
    LEFT JOIN owners o ON c.owner = o.owner
    JOIN cluster_members cm ON c.cluster_id = cm.cluster_id
"#;
pub const GET_CLUSTER_MEMBERS: &str = r#"
    SELECT operator_id
    FROM cluster_members
    WHERE cluster_id = ?1
"#;
pub const GET_CLUSTER_BY_VALIDATOR: &str = r#"
    SELECT
        c.cluster_id,
        c.owner,
        o.fee_recipient,
        c.liquidated
    FROM validators v
    JOIN clusters c ON v.cluster_id = c.cluster_id
    LEFT JOIN owners o ON c.owner = o.owner
    WHERE v.validator_pubkey = ?1
"#;

// Validator
pub const INSERT_VALIDATOR: &str = r#"
    INSERT INTO validators
        (validator_pubkey, cluster_id, validator_index, graffiti) 
    VALUES 
        (?1, ?2, ?3, ?4)
"#;
pub const DELETE_VALIDATOR: &str = r#"DELETE from validators WHERE validator_pubkey = ?1"#;
pub const GET_ALL_VALIDATORS: &str = r#"SELECT * FROM validators"#;
pub const GET_VALIDATOR: &str = r#"SELECT * FROM validators WHERE validator_pubkey = ?1"#;
// Shares
pub const INSERT_SHARE: &str = r#"
    INSERT INTO shares
        (validator_pubkey, cluster_id, operator_id, share_pubkey, encrypted_key)
    VALUES
        (?1, ?2, ?3, ?4, ?5)
"#;
pub const GET_SHARES: &str = r#"
    SELECT share_pubkey, encrypted_key, operator_id, cluster_id, validator_pubkey
    FROM shares WHERE operator_id = ?1
"#;
pub const GET_SHARE_PUBKEYS_FOR_VALIDATOR: &str = r#"
    SELECT operator_id, share_pubkey
    FROM shares WHERE validator_pubkey = ?1
"#;
pub const GET_SHARE_PUBKEYS_FOR_VALIDATOR_INDEX: &str = r#"
    SELECT s.operator_id, s.share_pubkey
    FROM shares s
    JOIN validators v ON v.validator_pubkey = s.validator_pubkey
    WHERE v.validator_index = ?1
"#;
pub const GET_OWN_SHARE: &str = r#"
    SELECT 1
    FROM shares
    WHERE validator_pubkey = ?1 AND operator_id = ?2
    LIMIT 1
"#;
// Misc Datta
pub const INSERT_OR_UPDATE_OWNER_FEE_RECIPIENT: &str = r#"
    INSERT INTO owners (owner, fee_recipient) VALUES (?1, ?2)
    ON CONFLICT (owner) DO UPDATE SET fee_recipient = ?2
"#;
pub const GET_OWNER_FEE_RECIPIENT: &str = r#"SELECT fee_recipient FROM owners WHERE owner = ?1"#;

pub const SET_GRAFFITI: &str = r#"UPDATE validators SET graffiti = ?1 WHERE validator_pubkey = ?2"#;
pub const SET_INDEX: &str = r#"
    UPDATE validators
    SET validator_index = ?1
    WHERE validator_pubkey = ?2
"#;

// Blocks
pub const UPDATE_BLOCK_NUMBER: &str = r#"UPDATE metadata SET block_number = ?1"#;
pub const GET_BLOCK_NUMBER: &str = r#"SELECT block_number FROM metadata"#;

// Nonce
pub const GET_ALL_NONCES: &str = r#"SELECT owner, nonce FROM owners"#;
pub const GET_NONCE: &str = r#"SELECT nonce FROM owners WHERE owner = ?1"#;
pub const BUMP_NONCE: &str = r#"
    INSERT INTO owners (owner, nonce) VALUES (?1, 0)
    ON CONFLICT (owner) DO UPDATE SET nonce = COALESCE(nonce + 1, 0)
"#;
