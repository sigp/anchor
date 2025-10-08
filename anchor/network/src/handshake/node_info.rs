use discv5::libp2p_identity::{Keypair, SigningError};
use serde::{Deserialize, Serialize};
use serde_json;
use subnet_service::{SubnetBits, SubnetId};
use thiserror::Error;

use crate::handshake::{
    envelope::{Envelope, make_unsigned},
    node_info::Error::Validation,
};

#[derive(Debug, Error)]
pub enum Error {
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Seal error: {0}")]
    Seal(#[from] SigningError),

    #[error("Validation error: {0}")]
    Validation(String),
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
pub struct NodeMetadata {
    #[serde(rename = "NodeVersion")]
    pub node_version: String,
    #[serde(rename = "ExecutionNode")]
    pub execution_node: String,
    #[serde(rename = "ConsensusNode")]
    pub consensus_node: String,
    #[serde(rename = "Subnets")]
    pub subnets: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
pub struct NodeInfo {
    pub network_id: String,
    pub metadata: Option<NodeMetadata>,
}

// This is the direct Rust equivalent to Go 'serializable' struct
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct Serializable {
    #[serde(rename = "Entries")]
    entries: Vec<String>,
}

impl NodeInfo {
    pub fn new(network_id: String, metadata: Option<NodeMetadata>) -> Self {
        NodeInfo {
            network_id,
            metadata,
        }
    }

    pub(crate) const DOMAIN: &'static str = "ssv";

    pub(crate) const CODEC: &'static [u8] = b"ssv/nodeinfo";

    /// Serialize `NodeInfo` to JSON bytes.
    fn marshal(&self) -> Result<Vec<u8>, Error> {
        let mut entries = vec![
            "".to_string(),                           // formerly forkVersion, now deprecated
            format!("0x{}", self.network_id.clone()), // network id
        ];

        if let Some(meta) = &self.metadata {
            entries.push(serde_json::to_string(meta)?);
        }

        // Serialize as JSON
        let ser = Serializable { entries };
        let data = serde_json::to_vec(&ser)?;
        Ok(data)
    }

    /// Deserialize `NodeInfo` from JSON bytes, replacing `self`.
    pub fn unmarshal(data: &[u8]) -> Result<NodeInfo, Error> {
        let ser: Serializable = serde_json::from_slice(data)?;
        if ser.entries.len() < 2 {
            return Err(Validation("node info must have at least 2 entries".into()));
        }
        // skip ser.entries[0]: old forkVersion
        let network_id = ser.entries[1]
            .clone()
            .strip_prefix("0x")
            .ok_or_else(|| Validation("network id must be prefixed with 0x".into()))?
            .to_string();

        let metadata = if ser.entries.len() >= 3 {
            let meta = serde_json::from_slice(ser.entries[2].as_bytes())?;
            Some(meta)
        } else {
            None
        };
        Ok(NodeInfo::new(network_id, metadata))
    }

    /// Seals a `Record` into an Envelope by:
    ///  1) marshalling record to bytes,
    ///  2) building "unsigned" data (domain + codec + payload),
    ///  3) signing,
    ///  4) storing into `Envelope`.
    pub fn seal(&self, keypair: &Keypair) -> Result<Envelope, Error> {
        let domain = Self::DOMAIN;
        let payload_type = Self::CODEC;

        let raw_payload = self.marshal()?;

        let unsigned = make_unsigned(domain.as_bytes(), payload_type, &raw_payload).unwrap();

        let sig = keypair.sign(&unsigned)?;

        let env = Envelope {
            public_key: keypair.public().encode_protobuf(),
            payload_type: payload_type.to_vec(),
            payload: raw_payload,
            signature: sig,
        };
        Ok(env)
    }
}

impl NodeMetadata {
    pub fn set_subscribed(&mut self, subnet: SubnetId, subscribed: bool) -> Result<(), Error> {
        let mut subnet_bits = SubnetBits::default();
        hex::decode_to_slice(&self.subnets, &mut subnet_bits)
            .map_err(|err| Validation(format!("Invalid subnet field: {err}")))?;
        let byte = subnet_bits
            .get_mut(*subnet as usize / 8)
            .ok_or_else(|| Validation(format!("Invalid subnet: {}", *subnet)))?;
        let mask = 1 << (*subnet % 8);
        if subscribed {
            *byte |= mask;
        } else {
            *byte &= !mask;
        }
        self.subnets = hex::encode(subnet_bits);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use libp2p::identity::Keypair;

    use crate::handshake::{
        envelope::Envelope,
        node_info::{NodeInfo, NodeMetadata},
    };

    const HOLESKY_WITH_PREFIX: &str = "0x00000502";
    const HOLESKY: &str = "00000502";

    #[test]
    fn test_node_info_seal_consume() {
        // Create a sample NodeInfo instance
        let node_info = NodeInfo::new(
            HOLESKY_WITH_PREFIX.to_string(),
            Some(NodeMetadata {
                node_version: "geth/x".to_string(),
                execution_node: "geth/x".to_string(),
                consensus_node: "prysm/x".to_string(),
                subnets: "00000000000000000000000000000000".to_string(),
            }),
        );

        // Marshal the NodeInfo into bytes and wrap it into an Envelope
        let envelope = node_info
            .seal(&Keypair::generate_secp256k1())
            .expect("Seal failed");

        let data = envelope.encode_to_vec().unwrap();

        let parsed_env = Envelope::parse_and_verify(&data).expect("Consume failed");
        let parsed_node_info = NodeInfo::unmarshal(&parsed_env.payload).expect("Unmarshal failed");

        assert_eq!(node_info, parsed_node_info);

        let encoded=
            hex::decode("0a2508021221037f3a82b9c83139f3e2c26850d688783ec779e7ca3f7824557d2e72af1f8ffeed120c7373762f6e6f6465696e666f1aaa017b22456e7472696573223a5b22222c22307830783030303030353032222c227b5c224e6f646556657273696f6e5c223a5c22676574682f785c222c5c22457865637574696f6e4e6f64655c223a5c22676574682f785c222c5c22436f6e73656e7375734e6f64655c223a5c22707279736d2f785c222c5c225375626e6574735c223a5c2230303030303030303030303030303030303030303030303030303030303030305c227d225d7d2a473045022100b362c2d4f1a32ee3d1503bfa83019d9273bdfed12ba9fced1c3e168848568b5202203e47cb6958f917613bf6022cf5b46ee1e1a628bee331e8ec1fa3acaa1f19d383").unwrap();

        let parsed_env = Envelope::parse_and_verify(&encoded).expect("Consume failed");
        let parsed_node_info = NodeInfo::unmarshal(&parsed_env.payload).expect("Unmarshal failed");

        assert_eq!(node_info, parsed_node_info);
    }

    #[test]
    fn test_node_info_marshal_unmarshal() {
        // The old serialized data from the Go code
        // (note the "Subnets":"ffffffffffffffffffffffffffffffff")
        let old_serialized_data = format!(
            r#"{{"Entries":["", "{HOLESKY_WITH_PREFIX}", "{{\"NodeVersion\":\"v0.1.12\",\"ExecutionNode\":\"geth/x\",\"ConsensusNode\":\"prysm/x\",\"Subnets\":\"ffffffffffffffffffffffffffffffff\"}}"]}}"#
        ).into_bytes();

        // The "current" NodeInfo data
        let current_data = NodeInfo {
            network_id: HOLESKY.to_string(),
            metadata: Some(NodeMetadata {
                node_version: "v0.1.12".into(),
                execution_node: "geth/x".into(),
                consensus_node: "prysm/x".into(),
                subnets: "ffffffffffffffffffffffffffffffff".into(),
            }),
        };

        // 1) Marshal current_data
        let data = current_data
            .marshal()
            .expect("marshal_record should succeed");

        // 2) Unmarshal into parsed_rec
        let parsed_rec = NodeInfo::unmarshal(&data).expect("unmarshal_record should succeed");

        // 3) Now unmarshal the old format data into the same struct
        let old_format =
            NodeInfo::unmarshal(&old_serialized_data).expect("unmarshal old data should succeed");

        // 4) Compare
        // The Go test checks reflect.DeepEqual(currentSerializedData, parsedRec)
        // We can do the same in Rust using assert_eq.
        assert_eq!(old_format, parsed_rec);
    }

    #[test]
    fn test_subnet_encoding_length() {
        use subnet_service::SubnetBits;

        // Test that hex encoding always produces exactly 32 characters
        let empty_subnets: SubnetBits = [0; 16];
        assert_eq!(hex::encode(empty_subnets).len(), 32);
        assert_eq!(hex::encode(empty_subnets), "00000000000000000000000000000000");

        let full_subnets: SubnetBits = [0xFF; 16];
        assert_eq!(hex::encode(full_subnets).len(), 32);
        assert_eq!(hex::encode(full_subnets), "ffffffffffffffffffffffffffffffff");
    }

    #[test]
    fn test_subnet_bit_encoding_matches_go_client() {
        use subnet_service::SubnetBits;

        // Test that our bit encoding matches the Go client format
        // Subnet 0 should set bit 0 of byte 0
        let mut subnet_bits: SubnetBits = [0; 16];
        subnet_bits[0] |= 1 << 0;
        assert_eq!(hex::encode(subnet_bits), "01000000000000000000000000000000");

        // Subnet 8 should set bit 0 of byte 1
        subnet_bits = [0; 16];
        subnet_bits[1] |= 1 << 0;
        assert_eq!(hex::encode(subnet_bits), "00010000000000000000000000000000");

        // Subnet 127 should set bit 7 of byte 15
        subnet_bits = [0; 16];
        subnet_bits[15] |= 1 << 7;
        assert_eq!(hex::encode(subnet_bits), "00000000000000000000000000000080");
    }

    #[test]
    fn test_set_subscribed_single_subnet() {
        use subnet_service::SubnetId;

        let mut metadata = NodeMetadata {
            node_version: "test".to_string(),
            execution_node: "test".to_string(),
            consensus_node: "test".to_string(),
            subnets: "00000000000000000000000000000000".to_string(),
        };

        // Subscribe to subnet 5
        metadata.set_subscribed(SubnetId::new(5), true).unwrap();

        // Verify bit 5 is set in byte 0
        let mut expected: [u8; 16] = [0; 16];
        expected[0] = 1 << 5;
        assert_eq!(metadata.subnets, hex::encode(expected));
    }

    #[test]
    fn test_set_subscribed_multiple_subnets() {
        use subnet_service::SubnetId;

        let mut metadata = NodeMetadata {
            node_version: "test".to_string(),
            execution_node: "test".to_string(),
            consensus_node: "test".to_string(),
            subnets: "00000000000000000000000000000000".to_string(),
        };

        // Subscribe to subnets 0, 8, and 127
        metadata.set_subscribed(SubnetId::new(0), true).unwrap();
        metadata.set_subscribed(SubnetId::new(8), true).unwrap();
        metadata.set_subscribed(SubnetId::new(127), true).unwrap();

        // Verify correct bits are set
        let mut expected: [u8; 16] = [0; 16];
        expected[0] = 1 << 0;  // subnet 0
        expected[1] = 1 << 0;  // subnet 8
        expected[15] = 1 << 7; // subnet 127
        assert_eq!(metadata.subnets, hex::encode(expected));
    }

    #[test]
    fn test_count_matching_subnets() {
        // Helper to count matching bits (same logic as in network.rs)
        fn count_matches(our_subnets: &str, their_subnets: &str) -> usize {
            let our_bytes = hex::decode(our_subnets).unwrap();
            let their_bytes = hex::decode(their_subnets).unwrap();
            our_bytes
                .iter()
                .zip(their_bytes.iter())
                .map(|(a, b)| (a & b).count_ones() as usize)
                .sum()
        }

        // No matches
        assert_eq!(
            count_matches(
                "01000000000000000000000000000000", // subnet 0
                "00010000000000000000000000000000"  // subnet 8
            ),
            0
        );

        // One match
        assert_eq!(
            count_matches(
                "01000000000000000000000000000000", // subnet 0
                "01000000000000000000000000000000"  // subnet 0
            ),
            1
        );

        // Multiple matches
        assert_eq!(
            count_matches(
                "03000000000000000000000000000000", // subnets 0 and 1
                "01000000000000000000000000000000"  // subnet 0
            ),
            1
        );

        // All match
        assert_eq!(
            count_matches(
                "ffffffffffffffffffffffffffffffff",
                "ffffffffffffffffffffffffffffffff"
            ),
            128
        );
    }
}
