use crate::handshake::envelope::{make_unsigned, Envelope};
use discv5::libp2p_identity::{Keypair, SigningError};
use serde::{Deserialize, Serialize};
use serde_json;

use crate::handshake::node_info::Error::Validation;
use thiserror::Error;

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
            "".to_string(),          // formerly forkVersion, now deprecated
            self.network_id.clone(), // network id
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
    pub fn unmarshal(&mut self, data: &[u8]) -> Result<(), Error> {
        let ser: Serializable = serde_json::from_slice(data)?;
        if ser.entries.len() < 2 {
            return Err(Validation("node info must have at least 2 entries".into()));
        }
        // skip ser.entries[0]: old forkVersion
        self.network_id = ser.entries[1].clone();
        if ser.entries.len() >= 3 {
            let meta = serde_json::from_slice(ser.entries[2].as_bytes())?;
            self.metadata = Some(meta);
        }
        Ok(())
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

#[cfg(test)]
mod tests {
    use crate::handshake::envelope::parse_envelope;
    use crate::handshake::node_info::{NodeInfo, NodeMetadata};
    use libp2p::identity::Keypair;

    #[test]
    fn test_node_info_seal_consume() {
        // Create a sample NodeInfo instance
        let node_info = NodeInfo::new(
            "holesky".to_string(),
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

        let parsed_env = parse_envelope(&data).expect("Consume failed");
        let mut parsed_node_info = NodeInfo::default();
        parsed_node_info
            .unmarshal(&parsed_env.payload)
            .expect("TODO: panic message");

        assert_eq!(node_info, parsed_node_info);

        let encoded=
            hex::decode("0a250802122102ba6a707dcec6c60ba2793d52123d34b22556964fc798d4aa88ffc41\
            a00e42407120c7373762f6e6f6465696e666f1aa5017b22456e7472696573223a5b22222c22686f6c65736b7\
            9222c227b5c224e6f646556657273696f6e5c223a5c22676574682f785c222c5c22457865637574696f6e4e6f64655c223a5c22676574682f785c222c5c22436f6e73656e7375734e6f64655c223a5c22707279736d2f785c222c5c225375626e6574735c223a5c2230303030303030303030303030303030303030303030303030303030303030305c227d225d7d2a473045022100b8a2a668113330369e74b86ec818a87009e2a351f7ee4c0e431e1f659dd1bc3f02202b1ebf418efa7fb0541f77703bea8563234a1b70b8391d43daa40b6e7c3fcc84").unwrap();

        let parsed_env = parse_envelope(&encoded).expect("Consume failed");
        let mut parsed_node_info = NodeInfo::default();
        parsed_node_info
            .unmarshal(&parsed_env.payload)
            .expect("TODO: panic message");

        assert_eq!(node_info, parsed_node_info);
    }

    #[test]
    fn test_node_info_marshal_unmarshal() {
        // The old serialized data from the Go code
        // (note the "Subnets":"ffffffffffffffffffffffffffffffff")
        let old_serialized_data = br#"{"Entries":["", "testnet", "{\"NodeVersion\":\"v0.1.12\",\"ExecutionNode\":\"geth/x\",\"ConsensusNode\":\"prysm/x\",\"Subnets\":\"ffffffffffffffffffffffffffffffff\"}"]}"#;

        // The "current" NodeInfo data
        let current_data = NodeInfo {
            network_id: "testnet".to_string(),
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
        let mut parsed_rec = NodeInfo::default();
        parsed_rec
            .unmarshal(&data)
            .expect("unmarshal_record should succeed");

        // 3) Now unmarshal the old format data into the same struct
        parsed_rec
            .unmarshal(old_serialized_data)
            .expect("unmarshal old data should succeed");

        // 4) Compare
        // The Go test checks reflect.DeepEqual(currentSerializedData, parsedRec)
        // We can do the same in Rust using assert_eq.
        assert_eq!(current_data, parsed_rec);
    }
}
