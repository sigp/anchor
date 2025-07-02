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
pub use types::{Epoch, Slot, VariableList};

// Shared constants used across message types
pub const RSA_SIGNATURE_SIZE: usize = 256;
pub const MAX_SIGNATURES: usize = 13;

// Helper that converts from OutOfBounds to a custom error variant.
#[macro_export]
macro_rules! vec_to_variable_list {
    ($v:expr, $error_variant:path) => {
        ssz_types::VariableList::new($v).map_err(|err| {
            if let ssz_types::Error::OutOfBounds { i, len } = err {
                $error_variant {
                    provided: i,
                    max: len,
                }
            } else {
                panic!(
                    "OutOfBounds is the only variant that should be returned by VariableList::new"
                )
            }
        })
    };
}
