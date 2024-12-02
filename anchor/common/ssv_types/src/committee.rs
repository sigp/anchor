use crate::util::parse_rsa;
use crate::{Operator, OperatorId};
use derive_more::{Deref, From};
use rsa::RsaPublicKey;
use types::Domain;

/// Unique identifier for a committee.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash, From, Deref)]
pub struct CommitteeId(u64);

/// Member of a SSV Committee. A CommitteeMember is just an operator that is part of the committee
/// a validator has chosen to distribute its keyshares to.
#[derive(Debug, Clone)]
pub struct CommitteeMember {
    /// Unique identifier for the operator
    pub operator_id: OperatorId,
    /// Unique identifier for the committee this member is a part of
    pub committee_id: CommitteeId,
    /// Base-64 encoded PEM RSA public key of the operator
    pub operator_public_key: RsaPublicKey,
    /// Number of nodes that are faulty/malicious in the committee
    pub faulty: u64,
    /// All of the operators that are a part of this committee
    pub members: Vec<Operator>,
    /// Signature domain
    pub domain: Domain,
}

impl CommitteeMember {
    /// Creates a new committee member from a PEM-encoded public key string
    pub fn new(
        pem_data: &str,
        operator_id: OperatorId,
        committee_id: CommitteeId,
        domain: Domain,
    ) -> Result<Self, String> {
        let rsa_pubkey = parse_rsa(pem_data)?;
        Ok(Self::new_with_pubkey(
            rsa_pubkey,
            operator_id,
            committee_id,
            domain,
        ))
    }

    /// Creates a new committee member from an existing RSA public key
    pub fn new_with_pubkey(
        rsa_pubkey: RsaPublicKey,
        operator_id: OperatorId,
        committee_id: CommitteeId,
        domain: Domain,
    ) -> Self {
        Self {
            operator_id,
            committee_id,
            operator_public_key: rsa_pubkey,
            faulty: 0,
            members: Vec::new(),
            domain,
        }
    }
}
