use ssz::{Decode, DecodeError};
use ssz_derive::{Decode, Encode};
use ssz_types::VariableList;
use tree_hash_derive::TreeHash;
use types::{EthSpec, Slot};

use crate::{consensus::BlindedExecutionPayloadEnvelope, message::SSVMessageDataLen};

/// Wire payload of `MsgType::SSVEnvelopeDisseminationMsgType` (SIP-94 §6).
///
/// The builder operator broadcasts the blinded form of its full envelope after the block
/// QBFT decides a self-build block; every operator validates it against its own block
/// decision and threshold-signs its root. `slot` stamps the duty slot for message
/// validation, mirroring `PartialSignatureMessages.slot`. The envelope rides as opaque
/// SSZ bytes so the wire container stays non-generic; the runner decodes it with its
/// `EthSpec` via [`Self::blinded_envelope`].
///
/// The inner list reuses the outer `SSVMessage.Data` bound: the blinded envelope is a few
/// hundred bytes in practice, and its Gloas progressive request lists carry no type-level
/// maximum to derive a tighter cap from.
#[derive(Debug, Clone, PartialEq, Eq, Encode, Decode, TreeHash)]
pub struct EnvelopeDissemination {
    pub slot: Slot,
    pub envelope: VariableList<u8, SSVMessageDataLen>,
}

impl EnvelopeDissemination {
    /// Decode the disseminated `BlindedExecutionPayloadEnvelope`.
    pub fn blinded_envelope<E: EthSpec>(
        &self,
    ) -> Result<BlindedExecutionPayloadEnvelope<E>, DecodeError> {
        BlindedExecutionPayloadEnvelope::from_ssz_bytes(&self.envelope)
    }
}

#[cfg(test)]
mod tests {
    use ssz::Encode;

    use super::*;

    #[test]
    fn envelope_dissemination_ssz_round_trip() {
        let original = EnvelopeDissemination {
            slot: Slot::new(42),
            envelope: VariableList::new(vec![1, 2, 3, 4]).unwrap(),
        };
        let decoded = EnvelopeDissemination::from_ssz_bytes(&original.as_ssz_bytes())
            .expect("round trip should decode");
        assert_eq!(decoded, original);
    }
}
