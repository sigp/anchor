use std::collections::HashMap;
use std::sync::LazyLock;

// Wrappers around various SQL statements used for interacting with the db
#[derive(Debug, Hash, Eq, PartialEq, Clone, Copy)]
pub(crate) enum SqlStatement {
    InsertOperator,  // Insert a new Operator in the database
    DeleteOperator,  // Delete an Operator from the database
    GetOperatorId,   // Get the ID of this operator from its public key
    GetAllOperators, // Get all of the Operators in the database

    InsertCluster,       // Insert a new Cluster into the database
    InsertClusterMember, // Insert a new Cluster Member into the database
    UpdateClusterStatus, // Update the active status of the cluster
    UpdateClusterFaulty, // Update the number of faulty Operators in the cluster
    GetAllClusters,      // Get all Clusters for state reconstruction
    GetClusterMembers,   // Get all Cluster Members for state reconstruction

    InsertValidator,  // Insert a Validator into the database
    DeleteValidator,  // Delete a Validator from the database
    GetAllValidators, // Get all Validators for state reconstruction

    InsertShare, // Insert a KeyShare into the database
    GetShares,   // Get the releveant keyshare for a validator

    UpdateFeeRecipient, // Update the fee recipient address for a cluster
    SetGraffiti,        // Update the Graffiti for a validator

    UpdateBlockNumber, // Update the last block that the database has processed
    GetBlockNumber,    // Get the last block that the database has processed

    GetAllNonces, // Fetch all the Nonce values for every Owner
    BumpNonce,    // Bump the nonce value for an Owner
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
    m.insert(SqlStatement::GetAllOperators, "SELECT * FROM operators");

    // Cluster
    m.insert(
        SqlStatement::InsertCluster,
        "INSERT OR IGNORE INTO clusters (cluster_id, owner, fee_recipient) VALUES (?1, ?2, ?3)",
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
        SqlStatement::UpdateClusterFaulty,
        "UPDATE clusters SET faulty = ?1 WHERE cluster_id = ?2",
    );
    m.insert(
        SqlStatement::GetAllClusters,
        "SELECT DISTINCT
            c.cluster_id,
            c.owner,
            c.fee_recipient,
            c.faulty,
            c.liquidated
        FROM clusters c
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
        SqlStatement::UpdateFeeRecipient,
        "UPDATE clusters SET fee_recipient = ?1 WHERE owner = ?2",
    );
    m.insert(
        SqlStatement::SetGraffiti,
        "UPDATE validators SET graffiti = ?1 WHERE validator_pubkey = ?2",
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
        SqlStatement::BumpNonce,
        "INSERT INTO nonce (owner, nonce) VALUES (?1, 0)
         ON CONFLICT (owner) DO UPDATE SET nonce = nonce + 1",
    );

    m
});
