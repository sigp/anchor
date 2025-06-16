// Operator
pub const INSERT_OPERATOR: &str = "
    INSERT INTO operators 
        (operator_id, public_key, owner_address)
    VALUES
        (?1, ?2, ?3)
";
pub const DELETE_OPERATOR: &str = "DELETE FROM operators WHERE operator_id = ?1";
pub const GET_OPERATOR_ID: &str = "SELECT operator_id FROM operators WHERE public_key = ?1";
pub const GET_OPERATOR_KEY: &str = "SELECT public_key FROM operators WHERE operator_id = ?1";
pub const GET_ALL_OPERATORS: &str = "SELECT * FROM operators";

// Cluster
pub const INSERT_CLUSTER: &str = "
    INSERT OR IGNORE INTO clusters 
        (cluster_id, owner) 
    VALUES 
        (?1, ?2)
";
pub const INSERT_CLUSTER_MEMBER: &str = "
    INSERT OR IGNORE INTO cluster_members
        (cluster_id, operator_id) 
    VALUES 
        (?1, ?2)
";
pub const UPDATE_CLUSTER_STATUS: &str = "UPDATE clusters SET liquidated = ?1 WHERE cluster_id = ?2";
pub const GET_ALL_CLUSTERS: &str = "
    SELECT DISTINCT
        c.cluster_id,
        c.owner,
        o.fee_recipient,
        c.liquidated
    FROM clusters c
    LEFT JOIN owners o ON c.owner = o.owner
    JOIN cluster_members cm ON c.cluster_id = cm.cluster_id
";
pub const GET_CLUSTER_MEMBERS: &str = "
    SELECT operator_id
    FROM cluster_members
    WHERE cluster_id = ?1
";

// Validator
pub const INSERT_VALIDATOR: &str = "
    INSERT INTO validators
        (validator_pubkey, cluster_id, validator_index, graffiti) 
    VALUES 
        (?1, ?2, ?3, ?4)
";
pub const DELETE_VALIDATOR: &str = "DELETE from validators WHERE validator_pubkey = ?1";
pub const GET_ALL_VALIDATORS: &str = "SELECT * FROM validators";

// Shares
pub const INSERT_SHARE: &str = "
    INSERT INTO shares
        (validator_pubkey, cluster_id, operator_id, share_pubkey, encrypted_key)
    VALUES
        (?1, ?2, ?3, ?4, ?5)
";
pub const GET_SHARES: &str = "
    SELECT share_pubkey, encrypted_key, operator_id, cluster_id, validator_pubkey
    FROM shares WHERE operator_id = ?1
";

// Misc Datta
pub const INSERT_OR_UPDATE_OWNER_FEE_RECIPIENT: &str = "
    INSERT INTO owners (owner, fee_recipient) VALUES (?1, ?2)
    ON CONFLICT (owner) DO UPDATE SET fee_recipient = ?2
";
pub const GET_OWNER_FEE_RECIPIENT: &str = "SELECT fee_recipient FROM owners WHERE owner = ?1";

pub const SET_GRAFFITI: &str = "UPDATE validators SET graffiti = ?1 WHERE validator_pubkey = ?2";
pub const SET_INDEX: &str = "
    UPDATE validators
    SET validator_index = ?1
    WHERE validator_pubkey = ?2
";

// Blocks
pub const UPDATE_BLOCK_NUMBER: &str = "UPDATE block SET block_number = ?1";
pub const GET_BLOCK_NUMBER: &str = "SELECT block_number FROM block";

// Nonce
pub const GET_ALL_NONCES: &str = "SELECT * FROM nonce";
pub const GET_NONCE: &str = "SELECT nonce FROM nonce WHERE owner = ?1";
pub const BUMP_NONCE: &str = "
    INSERT INTO nonce (owner, nonce) VALUES (?1, 0)
    ON CONFLICT (owner) DO UPDATE SET nonce = nonce + 1
";
