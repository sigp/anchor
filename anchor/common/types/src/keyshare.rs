
use types::{PublicKey, Address, Graffiti}; // ValidatorIndex
type ValidatorIndex = usize; // this will come from types
use crate::{OperatorID, CommitteeID};


pub type SharePublicKey = u64;

// A portion of a key given to an opeator
pub struct Keyshare {
    // Index of the validator
    pub validator_index: ValidatorIndex,
    // Public key of the validator
    pub validator_pubkey: PublicKey,
    // The public key of the share
    pub share_public_key: SharePublicKey,
    // The operator who owns this share
    pub operator_id: OperatorID,
    // The committee this share is a part of
    pub committe_id: CommitteeID,
    // Graffiti filed
    pub graffiti: Graffiti,
    // Fee RecipientAddress
    pub ethaddress: Address,
}
