mod beacon;
mod beacon_vote_encoding;
mod committee_member;
mod consensus_data_proposer;
mod deserializers;
mod duty;
mod encryption;
mod max_msg_size;
mod partial_sig_message;
mod partial_sig_message_encoding;
mod share_encoding;
mod signed_ssv_msg;
mod signed_ssv_msg_encoding;
mod ssv_msg;
mod ssv_msg_encoding;
mod ssz;
mod validator_consensus_data;
mod validator_consensus_data_encoding;

use std::fmt;

// Re-export test implementations
pub use beacon::*;
pub use beacon_vote_encoding::*;
pub use committee_member::*;
pub use consensus_data_proposer::*;
pub use duty::*;
pub use encryption::*;
pub use max_msg_size::*;
pub use partial_sig_message::*;
pub use partial_sig_message_encoding::*;
pub use share_encoding::*;
pub use signed_ssv_msg::*;
pub use signed_ssv_msg_encoding::*;
pub use ssv_msg::*;
pub use ssv_msg_encoding::*;
pub use ssz::*;
pub use validator_consensus_data::*;
pub use validator_consensus_data_encoding::*;

// Types-specific test type enumeration
#[derive(Eq, PartialEq, Hash, Debug)]
pub(crate) enum TypesSpecTestType {
    BeaconDepositData,
    BeaconVoteEncoding,
    CommitteeMember,
    ConsensusDataProposer,
    Duty,
    Encryption,
    MaxMsgSize,
    PartialSigMessage,
    PartialSigMessageEncoding,
    ShareEncoding,
    SignedSSVMsg,
    SignedSSVMsgEncoding,
    SSVMsg,
    SSVMsgEncoding,
    SSZ,
    ValidatorConsensusData,
    ValidatorConsensusDataEncoding,
}

impl TypesSpecTestType {
    // Determine if this is an encoding test
    pub fn is_encoding(&self) -> bool {
        match self {
            TypesSpecTestType::BeaconVoteEncoding
            // | TypesSpecTestType::ShareEncoding
            | TypesSpecTestType::PartialSigMessageEncoding
            | TypesSpecTestType::SignedSSVMsgEncoding
            | TypesSpecTestType::SSVMsgEncoding
            | TypesSpecTestType::ValidatorConsensusDataEncoding => true,
            _ => false,
        }
    }
}

// Contains specific identifier for the test file matching Go test naming
impl fmt::Display for TypesSpecTestType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            TypesSpecTestType::BeaconDepositData => write!(f, "beacon"),
            TypesSpecTestType::BeaconVoteEncoding => write!(f, "beaconvote"),
            TypesSpecTestType::CommitteeMember => write!(f, "committeemember"),
            TypesSpecTestType::ConsensusDataProposer => write!(f, "consensusdataproposer"),
            TypesSpecTestType::Duty => write!(f, "duty"),
            TypesSpecTestType::Encryption => write!(f, "encryption"),
            TypesSpecTestType::MaxMsgSize => write!(f, "maxmsgsize"),
            TypesSpecTestType::PartialSigMessage => write!(f, "partialsigmessage"),
            TypesSpecTestType::PartialSigMessageEncoding => write!(f, "partialsigmessage"),
            TypesSpecTestType::ShareEncoding => write!(f, "share"),
            TypesSpecTestType::SignedSSVMsg => write!(f, "signedssvmsg"),
            TypesSpecTestType::SignedSSVMsgEncoding => write!(f, "signedssvmsg"),
            TypesSpecTestType::SSVMsg => write!(f, "ssvmsg"),
            TypesSpecTestType::SSVMsgEncoding => write!(f, "ssvmsg"),
            TypesSpecTestType::SSZ => write!(f, "ssz"),
            TypesSpecTestType::ValidatorConsensusData => write!(f, "validatorconsensusdata"),
            TypesSpecTestType::ValidatorConsensusDataEncoding => {
                write!(f, "validatorconsensusdata")
            }
        }
    }
}

// Re-export the deserializers module as types_deserializers for backward compatibility
pub(crate) mod types_deserializers {
    pub(crate) use super::deserializers::*;
}
