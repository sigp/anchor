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
pub use ssz_types::{VariableList, typenum};
use typenum::Unsigned;
pub use types::{Epoch, Slot};

// Shared constants used across message types
pub const RSA_SIGNATURE_SIZE: usize = 256;
pub const MAX_SIGNATURES: usize = 13;

/// Maximum Byzantine/faulty members a committee of `members` can tolerate:
/// `f = ⌊(N − 1) / 3⌋`. Returns 0 for empty committees.
pub fn get_f(members: usize) -> usize {
    members.saturating_sub(1) / 3
}

/// Default QBFT quorum: `N − f`. Equivalent to `2f + 1` when `N = 3f + 1`
/// (SSV canonical sizes: 4, 7, 10, 13) and strictly larger for all other `N`.
/// Uses `saturating_sub` so `N = 0` returns 0 without panicking; small committees
/// (`N ≤ 3`) yield trivial unanimity quorums with `f = 0` (no Byzantine tolerance).
pub fn quorum_size(committee_size: usize) -> usize {
    committee_size.saturating_sub(get_f(committee_size))
}

/// Converts a Vec to VariableList, returning a custom error on failure.
pub fn try_to_variable_list<T, N, E, F>(vec: Vec<T>, error_fn: F) -> Result<VariableList<T, N>, E>
where
    N: Unsigned + Clone,
    F: FnOnce(usize, usize) -> E,
{
    let vec_len = vec.len();
    let max_len = N::to_usize();

    VariableList::new(vec).map_err(|_| error_fn(vec_len, max_len))
}
