use crate::util::parse_rsa;
use derive_more::{Deref, From};
use rsa::RsaPublicKey;
use std::cmp::Eq;
use std::fmt::Debug;
use std::hash::Hash;

/// Unique identifier for an Operator.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash, From, Deref)]
pub struct OperatorId(u64);

/// Client responsible for maintaining the overall health of the network.
#[derive(Debug, Clone)]
pub struct Operator {
    /// ID to uniquely identify this operator
    pub id: OperatorId,
    /// Base-64 encoded PEM RSA public key
    pub public_key: RsaPublicKey,
}

impl Operator {
    /// Creates a new operator from its OperatorId and PEM-encoded public key string
    pub fn new(pem_data: &str, operator_id: OperatorId) -> Result<Self, String> {
        let rsa_pubkey = parse_rsa(pem_data)?;
        Ok(Self::new_with_pubkey(rsa_pubkey, operator_id))
    }

    // Creates a new operator from an existing RSA public key and OperatorId
    pub fn new_with_pubkey(rsa_pubkey: RsaPublicKey, operator_id: OperatorId) -> Self {
        Self {
            id: operator_id,
            public_key: rsa_pubkey,
        }
    }
}

#[cfg(test)]
mod operator_tests {
    use super::*;

    #[test]
    fn operator_from_pubkey_and_id() {
        // Random valid operator public key and id: https://explorer.ssv.network/operators/1141
        let pem_data = "LS0tLS1CRUdJTiBSU0EgUFVCTElDIEtFWS0tLS0tCk1JSUJJakFOQmdrcWhraUc5dzBCQVFFRkFBT0NBUThBTUlJQkNnS0NBUUVBbFFmQVIzMEd4bFpacEwrNDByU0IKTEpSYlkwY2laZDBVMXhtTlp1bFB0NzZKQXJ5d2lia0Y4SFlQV2xkM3dERVdWZXZjRzRGVVBSZ0hDM1MrTHNuMwpVVC9TS280eE9nNFlnZ0xqbVVXQysyU3ZGRFhXYVFvdFRXYW5UU0drSEllNGFnTVNEYlUzOWhSMWdOSTJhY2NNCkVCcjU2eXpWcFMvKytkSk5xU002S1FQM3RnTU5ia2IvbEtlY0piTXM0ZWNRMTNkWUQwY3dFNFQxcEdTYUdhcEkKbFNaZ2lYd0cwSGFNTm5GUkt0OFlkZjNHaTFMRlh3Zlo5NHZFRjJMLzg3RCtidjdkSFVpSGRjRnh0Vm0rVjVvawo3VFptcnpVdXB2NWhKZ3lDVE9zc0xHOW1QSGNORnhEVDJ4NUJKZ2FFOVpJYnMrWVZ5a1k3UTE4VEhRS2lWcDFaCmp3SURBUUFCCi0tLS0tRU5EIFJTQSBQVUJMSUMgS0VZLS0tLS0K";
        let operator_id = 1141;

        let operator = Operator::new(pem_data, operator_id.into());
        assert!(operator.is_ok());

        if let Ok(op) = operator {
            assert_eq!(op.id.0, operator_id);
        }
    }
}
