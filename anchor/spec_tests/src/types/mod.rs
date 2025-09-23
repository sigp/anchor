mod beacon_vote_encoding;
mod consensus_data_proposer;
mod encryption;
mod partial_sig_message;
mod partial_sig_message_encoding;
mod signed_ssv_msg;
mod signed_ssv_msg_encoding;
mod ssv_msg;
mod ssv_msg_encoding;
mod validator_consensus_data_encoding;
use std::fmt;

// Re-export test implementations
pub use beacon_vote_encoding::*;
pub use consensus_data_proposer::*;
pub use encryption::*;
pub use partial_sig_message::*;
pub use partial_sig_message_encoding::*;
pub use signed_ssv_msg::*;
pub use signed_ssv_msg_encoding::*;
pub use ssv_msg::*;
pub use ssv_msg_encoding::*;
pub use validator_consensus_data_encoding::*;

// Types-specific test type enumeration
#[derive(Eq, PartialEq, Hash, Debug)]
pub(crate) enum TypesSpecTestType {
    BeaconVoteEncoding,
    ConsensusDataProposer,
    Encryption,
    PartialSigMessage,
    PartialSigMessageEncoding,
    SignedSSVMsg,
    SignedSSVMsgEncoding,
    SSVMsg,
    SSVMsgEncoding,
    ValidatorConsensusDataEncoding,
}

impl TypesSpecTestType {
    // Determine if this is an encoding test
    pub fn is_encoding(&self) -> bool {
        matches!(
            self,
            TypesSpecTestType::BeaconVoteEncoding
                | TypesSpecTestType::PartialSigMessageEncoding
                | TypesSpecTestType::SignedSSVMsgEncoding
                | TypesSpecTestType::SSVMsgEncoding
                | TypesSpecTestType::ValidatorConsensusDataEncoding
        )
    }
}

// Contains specific identifier for the test file matching Go test naming
impl fmt::Display for TypesSpecTestType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            TypesSpecTestType::BeaconVoteEncoding => write!(f, "beaconvote"),
            TypesSpecTestType::ConsensusDataProposer => write!(f, "consensusdataproposer"),
            TypesSpecTestType::Encryption => write!(f, "encryption"),
            TypesSpecTestType::PartialSigMessage => write!(f, "partialsigmessage"),
            TypesSpecTestType::PartialSigMessageEncoding => write!(f, "partialsigmessage"),
            TypesSpecTestType::SignedSSVMsg => write!(f, "signedssvmsg"),
            TypesSpecTestType::SignedSSVMsgEncoding => write!(f, "signedssvmsg"),
            TypesSpecTestType::SSVMsg => write!(f, "ssvmsg"),
            TypesSpecTestType::SSVMsgEncoding => write!(f, "ssvmsg"),
            TypesSpecTestType::ValidatorConsensusDataEncoding => {
                write!(f, "validatorconsensusdata")
            }
        }
    }
}
