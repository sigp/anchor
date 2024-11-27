use crate::{Operator, OperatorID, OperatorPublicKey};
use types::Domain;

/// Unique identifier for a committee.
pub type CommitteeID = u64;

/// Member of a SSV Committee. A `CommitteeMember` is just an operator that is part of the committee
/// a validator has chosen to distribute its keyshares to.
#[derive(Debug, Clone)]
pub struct CommitteeMember {
    // Unique identifier for the operator
    pub operator_id: OperatorID,
    // Unique identifier for the committee this member is a part of
    pub committee_id: CommitteeID,
    // Base-64 encoded PEM RSA public key of the operator
    pub share_public_key: OperatorPublicKey,
    // Number of nodes that are faulty/malicious in the committee
    pub faulty: u64,
    // All of the operators that are a part of this committee
    pub members: Vec<Operator>,
    // Signature domain
    pub domain: Domain,
}
