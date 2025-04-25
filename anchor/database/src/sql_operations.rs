use std::{collections::HashMap, sync::LazyLock};

// Wrappers around various SQL statements used for interacting with the db
#[derive(Debug, Hash, Eq, PartialEq, Clone, Copy)]
pub(crate) enum SqlStatement {
    InsertOperator,  // Insert a new Operator in the database
    DeleteOperator,  // Delete an Operator from the database
    GetOperatorId,   // Get the ID of this operator from its public key
    GetOperatorKey,  // Get the public key of an operator
    GetAllOperators, // Get all of the Operators in the database

    InsertCluster,       // Insert a new Cluster into the database
    InsertClusterMember, // Insert a new Cluster Member into the database
    UpdateClusterStatus, // Update the active status of the cluster
    GetAllClusters,      // Get all Clusters for state reconstruction
    GetClusterMembers,   // Get all Cluster Members for state reconstruction

    InsertValidator,  // Insert a Validator into the database
    DeleteValidator,  // Delete a Validator from the database
    GetAllValidators, // Get all Validators for state reconstruction

    InsertShare, // Insert a KeyShare into the database
    GetShares,   // Get the releveant keyshare for a validator

    InsertOrUpdateOwnerFeeRecipient, // Insert fee recipient or update it
    GetOwnerFeeRecipient,            // Get the fee recipient for an owner
    SetGraffiti,                     // Update the Graffiti for a validator
    SetIndex,                        // Set the Index for a validator

    UpdateBlockNumber, // Update the last block that the database has processed
    GetBlockNumber,    // Get the last block that the database has processed

    GetAllNonces, // Fetch all the Nonce values for every owner
    GetNonce,     // Get the Nonce for a specific owner
    BumpNonce,    // Bump the nonce value for an owner
}

pub(crate) static SQL: LazyLock<HashMap<SqlStatement, &'static str>> = LazyLock::new(|| {
    let mut m = HashMap::new();

    // Operator
    m.insert(
        SqlStatement::InsertOperator,
        "INSERT INTO operators (operator_id, public_key, owner_address) VALUES (?1, ?2, ?3)",
    );
    m.insert(
        SqlStatement::DeleteOperator,
        "DELETE FROM operators WHERE operator_id = ?1",
    );
    m.insert(
        SqlStatement::GetOperatorId,
        "SELECT operator_id FROM operators WHERE public_key = ?1",
    );
    m.insert(
        SqlStatement::GetOperatorKey,
        "SELECT public_key FROM operators WHERE operator_id = ?1",
    );
    m.insert(SqlStatement::GetAllOperators, "SELECT * FROM operators");

    // Cluster
    m.insert(
        SqlStatement::InsertCluster,
        "INSERT OR IGNORE INTO clusters (cluster_id, owner) VALUES (?1, ?2)",
    );
    m.insert(
        SqlStatement::InsertClusterMember,
        "INSERT OR IGNORE INTO cluster_members (cluster_id, operator_id) VALUES (?1, ?2)",
    );
    m.insert(
        SqlStatement::UpdateClusterStatus,
        "UPDATE clusters SET liquidated = ?1 WHERE cluster_id = ?2",
    );
    m.insert(
        SqlStatement::GetAllClusters,
        "SELECT DISTINCT
            c.cluster_id,
            c.owner,
            o.fee_recipient,
            c.liquidated
        FROM clusters c
        LEFT JOIN owners o ON c.owner = o.owner
        JOIN cluster_members cm ON c.cluster_id = cm.cluster_id",
    );
    m.insert(
        SqlStatement::GetClusterMembers,
        "SELECT operator_id FROM cluster_members WHERE cluster_id = ?1",
    );

    // Validator
    m.insert(
        SqlStatement::InsertValidator,
        "INSERT INTO validators (validator_pubkey, cluster_id, validator_index, graffiti) VALUES (?1, ?2, ?3, ?4)",
    );
    m.insert(
        SqlStatement::DeleteValidator,
        "DELETE from validators WHERE validator_pubkey = ?1",
    );
    m.insert(SqlStatement::GetAllValidators, "SELECT * FROM validators");

    // Shares
    m.insert(
        SqlStatement::InsertShare,
        "INSERT INTO shares
            (validator_pubkey, cluster_id, operator_id, share_pubkey, encrypted_key)
         VALUES
            (?1, ?2, ?3, ?4, ?5)",
    );
    m.insert(
        SqlStatement::GetShares,
        "SELECT share_pubkey, encrypted_key, operator_id, cluster_id, validator_pubkey FROM shares WHERE operator_id = ?1"
    );

    // Misc Datta
    m.insert(
        SqlStatement::InsertOrUpdateOwnerFeeRecipient,
        "INSERT INTO owners (owner, fee_recipient) VALUES (?1, ?2)
     ON CONFLICT (owner) DO UPDATE SET fee_recipient = ?2",
    );
    m.insert(
        SqlStatement::GetOwnerFeeRecipient,
        "SELECT fee_recipient FROM owners WHERE owner = ?1",
    );

    m.insert(
        SqlStatement::SetGraffiti,
        "UPDATE validators SET graffiti = ?1 WHERE validator_pubkey = ?2",
    );
    m.insert(
        SqlStatement::SetIndex,
        "UPDATE validators SET validator_index = ?1 WHERE validator_pubkey = ?2",
    );

    // Blocks
    m.insert(
        SqlStatement::UpdateBlockNumber,
        "UPDATE block SET block_number = ?1",
    );
    m.insert(
        SqlStatement::GetBlockNumber,
        "SELECT block_number FROM block",
    );

    // Nonce
    m.insert(SqlStatement::GetAllNonces, "SELECT * FROM nonce");
    m.insert(
        SqlStatement::GetNonce,
        "SELECT nonce FROM nonce WHERE owner = ?1",
    );
    m.insert(
        SqlStatement::BumpNonce,
        "INSERT INTO nonce (owner, nonce) VALUES (?1, 0)
         ON CONFLICT (owner) DO UPDATE SET nonce = nonce + 1",
    );

    m
});
