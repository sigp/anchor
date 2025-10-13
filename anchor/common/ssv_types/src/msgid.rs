use std::fmt::{Debug, Formatter};

use derive_more::{Display, From, Into};
use ssz::{Decode, DecodeError, Encode};
use types::{PublicKeyBytes, VariableList, typenum::U56};

use crate::{committee::CommitteeId, domain_type::DomainType};

const MESSAGE_ID_LEN: usize = 56;

#[derive(Debug, Display, Copy, Clone, Hash, Eq, PartialEq)]
pub enum Role {
    Committee,
    Aggregator,
    Proposer,
    SyncCommittee,
    ValidatorRegistration,
    VoluntaryExit,
}

impl From<Role> for [u8; 4] {
    fn from(value: Role) -> Self {
        match value {
            Role::Committee => [0, 0, 0, 0],
            Role::Aggregator => [1, 0, 0, 0],
            Role::Proposer => [2, 0, 0, 0],
            Role::SyncCommittee => [3, 0, 0, 0],
            Role::ValidatorRegistration => [4, 0, 0, 0],
            Role::VoluntaryExit => [5, 0, 0, 0],
        }
    }
}

impl TryFrom<&[u8]> for Role {
    type Error = DecodeError;

    fn try_from(value: &[u8]) -> Result<Self, Self::Error> {
        match value {
            [0, 0, 0, 0] => Ok(Role::Committee),
            [1, 0, 0, 0] => Ok(Role::Aggregator),
            [2, 0, 0, 0] => Ok(Role::Proposer),
            [3, 0, 0, 0] => Ok(Role::SyncCommittee),
            [4, 0, 0, 0] => Ok(Role::ValidatorRegistration),
            [5, 0, 0, 0] => Ok(Role::VoluntaryExit),
            _ => Err(DecodeError::NoMatchingVariant),
        }
    }
}

impl Role {
    pub fn max_round(self) -> Option<u64> {
        // as per https://github.com/ssvlabs/ssv/blob/6382d4b52ea5e0efd9378a5a00ef481f39d6234f/message/validation/consensus_validation.go#L370
        match self {
            Role::Committee | Role::Aggregator => Some(12),
            Role::Proposer | Role::SyncCommittee => Some(6),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub enum DutyExecutor {
    Committee(CommitteeId),
    Validator(PublicKeyBytes),
}

#[derive(Clone, Hash, Eq, PartialEq, From, Into)]
#[cfg_attr(feature = "arbitrary-fuzz", derive(arbitrary::Arbitrary))]
pub struct MessageId([u8; 56]);

impl Debug for MessageId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", hex::encode(self.0))
    }
}

impl MessageId {
    pub fn new(domain: &DomainType, role: Role, duty_executor: &DutyExecutor) -> Self {
        let mut id = [0; 56];
        id[0..4].copy_from_slice(&domain.0);
        id[4..8].copy_from_slice(&<[u8; 4]>::from(role));
        match duty_executor {
            DutyExecutor::Committee(committee_id) => {
                id[24..].copy_from_slice(committee_id.as_slice())
            }
            DutyExecutor::Validator(public_key) => {
                id[8..].copy_from_slice(public_key.as_serialized())
            }
        }

        MessageId(id)
    }

    pub fn domain(&self) -> DomainType {
        DomainType(
            self.0[0..4]
                .try_into()
                .expect("we know the slice has the correct length"),
        )
    }

    pub fn role(&self) -> Option<Role> {
        self.0[4..8].try_into().ok()
    }

    pub fn duty_executor(&self) -> Option<DutyExecutor> {
        // which kind of executor we need to get depends on the role
        match self.role()? {
            Role::Committee => self.0[24..].try_into().ok().map(DutyExecutor::Committee),
            _ => PublicKeyBytes::deserialize(&self.0[8..])
                .ok()
                .map(DutyExecutor::Validator),
        }
    }
}

impl AsRef<[u8]> for MessageId {
    fn as_ref(&self) -> &[u8] {
        self.0.as_ref()
    }
}

impl TryFrom<&[u8]> for MessageId {
    type Error = ();

    fn try_from(value: &[u8]) -> Result<Self, ()> {
        value.try_into().map(MessageId).map_err(|_| ())
    }
}

impl From<&MessageId> for VariableList<u8, U56> {
    fn from(value: &MessageId) -> Self {
        value.0.to_vec().into()
    }
}

impl Encode for MessageId {
    fn is_ssz_fixed_len() -> bool {
        true
    }

    fn ssz_append(&self, buf: &mut Vec<u8>) {
        buf.extend_from_slice(&self.0);
    }

    fn ssz_fixed_len() -> usize {
        MESSAGE_ID_LEN
    }

    fn ssz_bytes_len(&self) -> usize {
        MESSAGE_ID_LEN
    }
}

impl Decode for MessageId {
    fn is_ssz_fixed_len() -> bool {
        true
    }

    fn ssz_fixed_len() -> usize {
        MESSAGE_ID_LEN
    }

    fn from_ssz_bytes(bytes: &[u8]) -> Result<Self, DecodeError> {
        if bytes.len() != MESSAGE_ID_LEN {
            return Err(DecodeError::InvalidByteLength {
                len: bytes.len(),
                expected: MESSAGE_ID_LEN,
            });
        }
        let mut id = [0u8; MESSAGE_ID_LEN];
        id.copy_from_slice(bytes);
        Ok(MessageId(id))
    }
}
