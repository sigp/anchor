pub use cluster::{Cluster, ClusterId, ClusterMember, ValidatorIndex, ValidatorMetadata};
pub use committee::{CommitteeId, CommitteeInfo};
pub use operator::{Operator, OperatorId};
pub use share::Share;
mod cluster;
mod committee;
pub mod consensus;
pub mod domain_type;
pub mod message;
pub mod msgid;
mod operator;
pub mod partial_sig;
mod round;
mod share;
mod sql_conversions;
pub mod test_utils;

pub use indexmap::IndexSet;
pub use round::Round;
pub use share::ENCRYPTED_KEY_LENGTH;
use ssz_types::typenum::Unsigned;
pub use types::{Epoch, Slot, VariableList};

// Shared constants used across message types
pub const RSA_SIGNATURE_SIZE: usize = 256;
pub const MAX_SIGNATURES: usize = 13;

/// Converts a Vec to VariableList, returning a custom error on failure.
pub fn try_to_variable_list<T, N, E, F>(vec: Vec<T>, error_fn: F) -> Result<VariableList<T, N>, E>
where
    N: Unsigned + Clone,
    F: FnOnce(usize, usize) -> E,
{
    let vec_len = vec.len();
    let max_len = N::to_usize();

    if vec_len <= max_len {
        Ok(VariableList::from(vec))
    } else {
        Err(error_fn(vec_len, max_len))
    }
}
