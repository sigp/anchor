use crate::domain_type::DomainType;
use ssz::{Decode, DecodeError, Encode};

const MESSAGE_ID_LEN: usize = 56;

#[derive(Debug, Copy, Clone, Hash, Eq, PartialEq)]
pub enum Role {
    Committee,
    Aggregator,
    Proposer,
    SyncCommittee,
}

impl From<Role> for [u8; 4] {
    fn from(value: Role) -> Self {
        match value {
            Role::Committee => [0, 0, 0, 0],
            Role::Aggregator => [1, 0, 0, 0],
            Role::Proposer => [2, 0, 0, 0],
            Role::SyncCommittee => [3, 0, 0, 0],
        }
    }
}

impl TryFrom<&[u8]> for Role {
    type Error = ();

    fn try_from(value: &[u8]) -> Result<Self, Self::Error> {
        match value {
            [0, 0, 0, 0] => Ok(Role::Committee),
            [1, 0, 0, 0] => Ok(Role::Aggregator),
            [2, 0, 0, 0] => Ok(Role::Proposer),
            [3, 0, 0, 0] => Ok(Role::SyncCommittee),
            _ => Err(()),
        }
    }
}

#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub enum Executor {
    Committee([u8; 32]),
    Validator([u8; 48]),
}

#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub struct MessageId([u8; 56]);

impl MessageId {
    pub fn new(domain: &DomainType, role: Role, duty_executor: &Executor) -> Self {
        let mut id = [0; 56];
        id[0..4].copy_from_slice(&domain.0);
        id[4..8].copy_from_slice(&<[u8; 4]>::from(role));
        match duty_executor {
            Executor::Committee(slice) => id[24..].copy_from_slice(slice),
            Executor::Validator(slice) => id[8..].copy_from_slice(slice),
        }

        MessageId(id)
    }
}

impl AsRef<[u8]> for MessageId {
    fn as_ref(&self) -> &[u8] {
        self.0.as_ref()
    }
}

impl From<[u8; MESSAGE_ID_LEN]> for MessageId {
    fn from(value: [u8; MESSAGE_ID_LEN]) -> Self {
        MessageId(value)
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
