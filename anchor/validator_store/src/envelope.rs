use tree_hash::TreeHash;
use types::{EthSpec, ExecutionPayloadEnvelope, Hash256, SignedRoot};

/// Progressive root view of an envelope. Only its signing root is sent to peers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, tree_hash_derive::TreeHash)]
#[tree_hash(
    struct_behaviour = "progressive_container",
    active_fields(1, 1, 1, 1, 1)
)]
pub(super) struct BlindedExecutionPayloadEnvelope {
    pub payload_root: Hash256,
    pub execution_requests_root: Hash256,
    pub builder_index: u64,
    pub beacon_block_root: Hash256,
    pub parent_beacon_block_root: Hash256,
}

impl SignedRoot for BlindedExecutionPayloadEnvelope {}

impl BlindedExecutionPayloadEnvelope {
    pub fn from_full<E: EthSpec>(full: &ExecutionPayloadEnvelope<E>) -> Self {
        Self {
            payload_root: full.payload.tree_hash_root(),
            execution_requests_root: full.execution_requests.tree_hash_root(),
            builder_index: full.builder_index,
            beacon_block_root: full.beacon_block_root,
            parent_beacon_block_root: full.parent_beacon_block_root,
        }
    }
}
