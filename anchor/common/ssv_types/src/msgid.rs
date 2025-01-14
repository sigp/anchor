// todo probably move that to its own thing
#[derive(Debug, Copy, Clone, Hash, Eq, PartialEq)]
pub struct Domain([u8; 4]);
pub const MAINNET_DOMAIN: Domain = Domain([0, 0, 0, 1]);
pub const HOLESKY_DOMAIN: Domain = Domain([0, 0, 5, 2]);

#[derive(Debug, Copy, Clone, Hash, Eq, PartialEq)]
pub enum Role {
    Committee,
    Aggregator,
    Proposer,
    SyncCommittee,
}

impl Role {
    fn into_message_id_bytes(self) -> [u8; 4] {
        match self {
            Role::Committee => [0, 0, 0, 0],
            Role::Aggregator => [1, 0, 0, 0],
            Role::Proposer => [2, 0, 0, 0],
            Role::SyncCommittee => [3, 0, 0, 0],
        }
    }
}

#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub enum Executor {
    Committee([u8; 32]),
    Validator([u8; 48]),
}

#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub struct MsgId([u8; 56]);

impl MsgId {
    pub fn new(domain: &Domain, role: Role, duty_executor: &Executor) -> Self {
        let mut id = [0; 56];
        id[0..4].copy_from_slice(&domain.0);
        id[4..8].copy_from_slice(&role.into_message_id_bytes());
        match duty_executor {
            Executor::Committee(slice) => id[24..].copy_from_slice(slice),
            Executor::Validator(slice) => id[8..].copy_from_slice(slice),
        }

        MsgId(id)
    }
}
