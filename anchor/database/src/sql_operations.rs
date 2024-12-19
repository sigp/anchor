use std::collections::HashMap;
use std::sync::LazyLock;

// Wrappers around various SQL statements used for interacting with the db
#[derive(Debug, Hash, Eq, PartialEq, Clone, Copy)]
pub(crate) enum SqlStatement {
    InsertOperator,
    DeleteOperator,
    GetOperatorId,
    GetAllOperators,

    InsertCluster,
    InsertClusterMember,
    UpdateClusterStatus,
    UpdateClusterFaulty,
    DeleteCluster,
    GetAllClusters,
    GetClusterMembers,

    DeleteValidator,
    InsertShare,
    InsertValidator,
    UpdateFeeRecipient,
    SetGraffiti,
    SetValidatorIndex,

    UpdateBlockNumber,
    GetBlockNumber,

    GetShareAndValidator,
}

pub(crate) static SQL: LazyLock<HashMap<SqlStatement, &'static str>> = LazyLock::new(|| {
    let mut m = HashMap::new();
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
    m.insert(
        SqlStatement::InsertCluster,
        "INSERT OR IGNORE INTO clusters (cluster_id, owner, fee_recipient) VALUES (?1, ?2, ?3)",
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
        SqlStatement::InsertClusterMember,
        "INSERT OR IGNORE INTO cluster_members (cluster_id, operator_id) VALUES (?1, ?2)",
    );
    m.insert(
        SqlStatement::DeleteCluster,
        "DELETE FROM clusters WHERE cluster_id = ?1",
    );

    m.insert(
        SqlStatement::DeleteValidator,
        "DELETE from validators WHERE validator_pubkey = ?1",
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
        JOIN cluster_members cm ON c.cluster_id = cm.cluster_id
        WHERE cm.operator_id = ?",
    );
    m.insert(
        SqlStatement::GetClusterMembers,
        "SELECT operator_id FROM cluster_members WHERE cluster_id = ?1",
    );
    m.insert(SqlStatement::InsertShare,
        "INSERT INTO shares (validator_pubkey, cluster_id, operator_id, share_pubkey, encrypted_key) VALUES (?1, ?2, ?3, ?4, ?5)");
    m.insert(
        SqlStatement::InsertValidator,
        "INSERT INTO validators (validator_pubkey, cluster_id, validator_index, graffiti) VALUES (?1, ?2, ?3, ?4)",
    );
    m.insert(
        SqlStatement::UpdateFeeRecipient,
        "UPDATE clusters SET fee_recipient = ?1 WHERE owner = ?2",
    );
    m.insert(
        SqlStatement::SetGraffiti,
        "UPDATE validators SET graffiti = ?1 WHERE validator_pubkey = ?2",
    );
    m.insert(
        SqlStatement::SetValidatorIndex,
        "UPDATE validators SET validator_index = ?1 WHERE validator_pubkey = ?2",
    );
    m.insert(
        SqlStatement::UpdateBlockNumber,
        "UPDATE block SET block_number = ?1",
    );
    m.insert(
        SqlStatement::GetBlockNumber,
        "SELECT block_number FROM block",
    );
    m.insert(
        SqlStatement::GetShareAndValidator,
        "SELECT
            v.validator_pubkey,
            v.cluster_id,
            v.validator_index,
            v.graffiti,
            s.share_pubkey,
            s.encrypted_key,
            s.operator_id
        FROM validators v
        JOIN shares s ON v.validator_pubkey = s.validator_pubkey
        WHERE s.operator_id = ?1",
    );
    m
});
